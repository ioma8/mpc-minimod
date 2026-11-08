/* Live looper engine.
 *
 * Records pad loops from the firmware's audio input stream (the source the
 * sampler itself hears) and mixes them into the firmware's audio output. Both
 * hooks run on audio threads, so nothing here allocates, locks or touches the
 * app's model: the loop pool is mapped once when the view opens and the state
 * machine is a handful of atomics.
 *
 * The output side is the clock: every played frame advances one transport
 * counter, and a loop is an exact count of those frames. That keeps playback
 * and recording in the same time base even when the two audio streams drift.
 */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#include "looper.h"

#define LP_RATE 44100u
#define LP_SECONDS 8u
#define LP_MAX_FRAMES (LP_RATE * LP_SECONDS)
#define LP_RING 32768u
#define LP_BLOCK 2048u
#define LP_GAIN 0.85f
#define LP_CLICK 420u

static float *g_pool; /* LOOPER_PADS * LP_MAX_FRAMES mono floats */
static volatile int g_open;

static volatile int g_shift;
static volatile int g_sync;
static volatile unsigned g_sync_frames;
static volatile uint64_t g_pulse_epoch;
static volatile uint64_t g_t; /* played frames since open */

struct pad {
    volatile int state;
    volatile unsigned len; /* closed length, frames */
    volatile unsigned rec; /* frames captured, writer: audio thread only */
    volatile uint64_t origin;
    volatile int touched;
};

static struct pad g_pads[LOOPER_PADS];

/* mono input ring: input thread writes, output thread reads */
static float g_ring[LP_RING];
static volatile unsigned g_ring_w;
static volatile unsigned g_ring_r;
static volatile int g_ring_over;

static volatile int g_rec_count; /* pads in REC or DUB */
static volatile unsigned g_input_level; /* float bits */
static volatile unsigned g_rec_total;   /* frames captured, for telemetry */
static volatile unsigned g_blocks;      /* mixed blocks, for telemetry */
static volatile unsigned g_mix_peak;    /* float bits, for telemetry */
static volatile unsigned g_input_blocks;

static volatile long long g_msg_until;
static char g_msg[40];

static long long now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void say(const char *text)
{
    size_t n = strlen(text);
    if (n >= sizeof g_msg)
        n = sizeof g_msg - 1;
    memcpy(g_msg, text, n);
    g_msg[n] = 0;
    g_msg_until = now_ms() + 2500;
}

static float *pad_buf(int pad)
{
    return g_pool ? g_pool + (size_t)pad * LP_MAX_FRAMES : 0;
}

/* Nearest sync multiple, rounded up when snapping would drop more than 1/8 of
 * a pulse, so recorded material is never silently truncated. */
static unsigned quantize(unsigned frames)
{
    unsigned q = g_sync_frames;
    if (!g_sync || !q || !frames)
        return frames;
    unsigned m = (frames + q / 2) / q;
    if (m < 1)
        m = 1;
    uint64_t target = (uint64_t)m * q;
    if (target < frames && frames - target > q / 8)
        target += q;
    while (target > LP_MAX_FRAMES && m > 1) {
        --m;
        target = (uint64_t)m * q;
    }
    if (target > LP_MAX_FRAMES)
        target = (LP_MAX_FRAMES / q) * q;
    return (unsigned)target;
}

static void finish_pad(struct pad *p)
{
    int st = p->state;
    if (st != LOOP_REC && st != LOOP_DUB)
        return;
    if (st == LOOP_REC) {
        unsigned n = p->rec;
        unsigned len = quantize(n);
        p->len = len ? len : n;
        p->rec = n;
        p->state = LOOP_PLAY;
    } else {
        p->state = LOOP_PLAY;
    }
}

int looper_is_open(void)
{
    return g_open;
}

int looper_activate(void)
{
    if (g_open)
        return 1;
    if (!g_pool) {
        void *m = mmap(0, (size_t)LOOPER_PADS * LP_MAX_FRAMES * sizeof(float),
                       PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (m == MAP_FAILED)
            return 0;
        g_pool = m;
    }
    for (int i = 0; i < LOOPER_PADS; ++i) {
        g_pads[i].state = g_pads[i].len ? LOOP_STOP : LOOP_IDLE;
        g_pads[i].rec = 0;
        g_pads[i].origin = 0;
        g_pads[i].touched = 0;
    }
    g_ring_w = g_ring_r = 0;
    g_ring_over = 0;
    g_rec_count = 0;
    g_input_level = 0;
    g_t = 0;
    g_shift = 0;
    g_sync = 0;
    g_sync_frames = 0;
    g_pulse_epoch = 0;
    g_msg[0] = 0;
    g_open = 1;
    say("PAD: record   SHIFT+PAD: overdub");
    return 1;
}

void looper_close(void)
{
    if (!g_open)
        return;
    for (int i = 0; i < LOOPER_PADS; ++i) {
        if (g_pads[i].state == LOOP_REC || g_pads[i].state == LOOP_DUB)
            finish_pad(&g_pads[i]);
        if (g_pads[i].state == LOOP_PLAY || g_pads[i].state == LOOP_DUB)
            g_pads[i].state = LOOP_STOP;
    }
    g_rec_count = 0;
    g_sync = 0;
    g_input_level = 0;
    g_open = 0;
}

static void start_record(struct pad *p, int pad)
{
    float *buf = pad_buf(pad);
    if (!buf)
        return;
    /* Silence the previous take before the audio thread can see the new state,
     * and touch the pages on this (non-audio) thread. */
    p->state = LOOP_IDLE;
    __atomic_thread_fence(__ATOMIC_SEQ_CST);
    memset(buf, 0, (size_t)LP_MAX_FRAMES * sizeof(float));
    p->len = 0;
    p->rec = 0;
    p->origin = g_t;
    p->state = LOOP_REC;
    __atomic_add_fetch(&g_rec_count, 1, __ATOMIC_SEQ_CST);
}

static void start_overdub(struct pad *p)
{
    if (!p->len)
        return;
    p->state = LOOP_DUB;
    __atomic_add_fetch(&g_rec_count, 1, __ATOMIC_SEQ_CST);
}

int looper_pad(int pad, int shift)
{
    if (!g_open)
        return 0;
    if (pad < 0 || pad >= LOOPER_PADS)
        return 1;
    struct pad *p = &g_pads[pad];
    int st = p->state;
    if (shift) {
        if (st == LOOP_PLAY || st == LOOP_STOP)
            start_overdub(p);
        else if (st == LOOP_DUB || st == LOOP_REC)
            finish_pad(p);
        else if (st == LOOP_IDLE)
            start_record(p, pad);
        return 1;
    }
    switch (st) {
    case LOOP_IDLE:
        start_record(p, pad);
        break;
    case LOOP_REC:
        finish_pad(p);
        break;
    case LOOP_DUB:
        finish_pad(p);
        break;
    case LOOP_PLAY:
        p->state = LOOP_STOP;
        break;
    case LOOP_STOP:
        p->origin = g_t;
        p->state = LOOP_PLAY;
        break;
    }
    return 1;
}

int looper_button(unsigned code, int down, int shift)
{
    if (!g_open)
        return 0;
    g_shift = shift;
    if (code == 0x6f) { /* DATA: SHIFT+DATA closes */
        if (down && shift)
            looper_close();
        return 1;
    }
    if (code == 0x4a) { /* LOOP: sync pulse, SHIFT+LOOP: clear */
        if (down && shift) {
            for (int i = 0; i < LOOPER_PADS; ++i) {
                g_pads[i].state = LOOP_IDLE;
                g_pads[i].len = 0;
                g_pads[i].rec = 0;
                g_pads[i].origin = 0;
            }
            g_rec_count = 0;
            g_sync = 0;
            g_sync_frames = 0;
            say("ALL LOOPS CLEARED");
        } else if (down) {
            if (g_sync) {
                g_sync = 0;
                say("SYNC off");
            } else {
                unsigned best = 0;
                uint64_t latest = 0;
                for (int i = 0; i < LOOPER_PADS; ++i)
                    if (g_pads[i].len && g_pads[i].origin >= latest) {
                        latest = g_pads[i].origin;
                        best = g_pads[i].len;
                    }
                if (!best) {
                    say("SYNC: record a loop first");
                } else {
                    g_sync_frames = best;
                    g_pulse_epoch = g_t;
                    g_sync = 1;
                    char line[40];
                    unsigned ms = (unsigned)((uint64_t)best * 1000u / LP_RATE);
                    snprintf(line, sizeof line, "SYNC %.2fs", ms / 1000.0);
                    say(line);
                }
            }
        }
        return 1;
    }
    return 1; /* every other control is blocked while the view is open */
}

/* ------------------------------------------------------------ audio input */
/* Shared tail: one block of mono input, already in float. */
static void input_block(const float *mono, unsigned n, float peak)
{
    __atomic_add_fetch(&g_input_blocks, 1, __ATOMIC_RELAXED);
    int rec = __atomic_load_n(&g_rec_count, __ATOMIC_ACQUIRE);
    unsigned w = g_ring_w;
    for (unsigned i = 0; i < n; ++i) {
        if (rec) {
            unsigned next = w + 1;
            if (next - g_ring_r < LP_RING) {
                g_ring[w % LP_RING] = mono[i];
                w = next;
            } else {
                g_ring_over = 1;
            }
        }
    }
    if (rec)
        g_ring_w = w;
    float prev;
    unsigned pb = g_input_level;
    memcpy(&prev, &pb, sizeof prev);
    if (peak > prev * 0.98f) {
        unsigned nb;
        memcpy(&nb, &peak, sizeof nb);
        g_input_level = nb;
    }
}

void looper_input(const float *const *channels, int count, unsigned frames)
{
    if (!g_open || !channels || count < 1 || !frames)
        return;
    const float *l = channels[0];
    const float *r = count > 1 ? channels[1] : 0;
    if (!l)
        return;
    unsigned n = frames > LP_BLOCK ? LP_BLOCK : frames;
    float tmp[LP_BLOCK];
    float peak = 0.f;
    for (unsigned i = 0; i < n; ++i) {
        float v = r ? (l[i] + r[i]) * 0.5f : l[i];
        float a = v < 0.f ? -v : v;
        if (a > peak)
            peak = a;
        tmp[i] = v;
    }
    input_block(tmp, n, peak);
}

/* The firmware's input thread reads S32 planar from ALSA; convert it the same
 * way the app does and feed the same path. */
void looper_input_s32(const int32_t *const *channels, int count, unsigned frames)
{
    if (!g_open || !channels || count < 1 || !channels[0] || !frames)
        return;
    const int32_t *l = channels[0];
    unsigned n = frames > LP_BLOCK ? LP_BLOCK : frames;
    float tmp[LP_BLOCK];
    float peak = 0.f;
    for (unsigned i = 0; i < n; ++i) {
        float v = (float)l[i] * (1.0f / 2147483648.0f);
        float a = v < 0.f ? -v : v;
        if (a > peak)
            peak = a;
        tmp[i] = v;
    }
    input_block(tmp, n, peak);
}

/* Interleaved S32 capture (snd_pcm_readi): the firmware's input stream is
 * RW_INTERLEAVED 4-channel S32_LE. Channels 0/1 carry the selected source. */
void looper_input_s32_interleaved(const int32_t *buf, unsigned channels,
                                  unsigned frames)
{
    if (!g_open || !buf || !channels || !frames)
        return;
    unsigned n = frames > LP_BLOCK ? LP_BLOCK : frames;
    float tmp[LP_BLOCK];
    float peak = 0.f;
    for (unsigned i = 0; i < n; ++i) {
        float v;
        if (channels >= 2)
            v = ((float)buf[i * channels] + (float)buf[i * channels + 1]) *
                (0.5f / 2147483648.0f);
        else
            v = (float)buf[i * channels] * (1.0f / 2147483648.0f);
        float a = v < 0.f ? -v : v;
        if (a > peak)
            peak = a;
        tmp[i] = v;
    }
    input_block(tmp, n, peak);
}

/* ----------------------------------------------------------- audio output */
static float click_sample(uint64_t phase)
{
    if (phase >= LP_CLICK)
        return 0.f;
    float env = expf(-(float)phase / 70.f);
    return sinf(6.2831853f * 1000.f * (float)phase / (float)LP_RATE) * env * 0.35f;
}

/* Mix the loops into the interleaved S32 block the firmware is about to hand to
 * the device. This is the one point every audio path goes through, so it does
 * not matter which device object renders the app's own sound. */
void looper_render_s32(int32_t *buf, unsigned channels, unsigned frames)
{
    if (!g_open || !buf || !channels || !frames)
        return;
    __atomic_add_fetch(&g_blocks, 1, __ATOMIC_RELAXED);
    unsigned done = 0;
    int rec = __atomic_load_n(&g_rec_count, __ATOMIC_ACQUIRE);
    while (done < frames) {
        unsigned n = frames - done;
        if (n > LP_BLOCK)
            n = LP_BLOCK;
        float in[LP_BLOCK];
        unsigned got = 0;
        if (rec) {
            unsigned r = g_ring_r;
            unsigned w = __atomic_load_n(&g_ring_w, __ATOMIC_ACQUIRE);
            unsigned avail = w - r;
            if (avail > n * 4) {
                unsigned skip = avail - n * 2;
                r += skip;
                avail -= skip;
                g_ring_over = 1;
            }
            got = avail < n ? avail : n;
            for (unsigned i = 0; i < got; ++i)
                in[i] = g_ring[(r + i) % LP_RING];
            g_ring_r = r + got;
        }
        for (unsigned i = got; i < n; ++i)
            in[i] = 0.f;

        uint64_t t = g_t;
        float add[LP_BLOCK];
        for (unsigned i = 0; i < n; ++i)
            add[i] = 0.f;
        for (int p = 0; p < LOOPER_PADS; ++p) {
            struct pad *pd = &g_pads[p];
            int st = pd->state;
            if (st == LOOP_PLAY || st == LOOP_DUB) {
                unsigned len = pd->len;
                float *b = pad_buf(p);
                if (len && b) {
                    uint64_t base = t - pd->origin;
                    for (unsigned i = 0; i < n; ++i)
                        add[i] += b[(base + i) % len] * LP_GAIN;
                }
                if (st == LOOP_DUB && len && b) {
                    uint64_t base = t - pd->origin;
                    for (unsigned i = 0; i < n; ++i)
                        b[(base + i) % len] += in[i];
                }
            } else if (st == LOOP_REC) {
                float *b = pad_buf(p);
                unsigned rn = pd->rec;
                if (b) {
                    unsigned room = LP_MAX_FRAMES - rn;
                    unsigned cp = n < room ? n : room;
                    memcpy(b + rn, in, (size_t)cp * sizeof(float));
                    pd->rec = rn + cp;
                    __atomic_add_fetch(&g_rec_total, cp, __ATOMIC_RELAXED);
                    if (rn + cp >= LP_MAX_FRAMES)
                        finish_pad(pd);
                }
            }
        }
        if (g_sync) {
            unsigned q = g_sync_frames;
            if (q) {
                uint64_t e = g_pulse_epoch;
                for (unsigned i = 0; i < n; ++i)
                    add[i] += click_sample((t + i - e) % q);
            }
        }
        float peak = 0.f;
        for (unsigned i = 0; i < n; ++i) {
            int32_t *frame = buf + (size_t)(done + i) * channels;
            for (unsigned c = 0; c < channels; ++c) {
                float v = (float)frame[c] * (1.0f / 2147483648.0f) + add[i];
                if (v > 1.f)
                    v = 1.f;
                else if (v < -1.f)
                    v = -1.f;
                float a = v < 0.f ? -v : v;
                if (a > peak)
                    peak = a;
                frame[c] = (int32_t)(v * 2147483647.0f);
            }
        }
        unsigned pb = g_mix_peak;
        float prev;
        memcpy(&prev, &pb, sizeof prev);
        if (peak > prev) {
            unsigned nb;
            memcpy(&nb, &peak, sizeof nb);
            g_mix_peak = nb;
        }
        g_t = t + n;
        done += n;
    }
}

/* --------------------------------------------------------------- snapshot */
int looper_snapshot(struct looper_view *view)
{
    if (!view)
        return 0;
    memset(view, 0, sizeof *view);
    view->open = g_open;
    if (!g_open)
        return 0;
    view->sync = g_sync;
    view->sync_frames = g_sync_frames;
    view->pulse_phase = 0xffffffffu;
    if (g_sync && g_sync_frames)
        view->pulse_phase = (unsigned)((g_t - g_pulse_epoch) % g_sync_frames);
    unsigned lb = g_input_level;
    float lv;
    memcpy(&lv, &lb, sizeof lv);
    view->input = lv > 1.f ? 1.f : lv;
    view->recording_pad = -1;
    uint64_t t = g_t;
    for (int i = 0; i < LOOPER_PADS; ++i) {
        struct pad *p = &g_pads[i];
        struct looper_pad_view *v = &view->pads[i];
        v->state = (unsigned char)p->state;
        v->len_frames = p->len;
        v->rec_frames = p->rec;
        if ((p->state == LOOP_PLAY || p->state == LOOP_DUB) && p->len)
            v->pos = (unsigned)((t - p->origin) % p->len);
        if (p->state == LOOP_REC || p->state == LOOP_DUB)
            if (view->recording_pad < 0)
                view->recording_pad = i;
    }
    if (g_msg[0] && now_ms() < g_msg_until)
        memcpy(view->message, g_msg, sizeof view->message - 1);
    return 1;
}
