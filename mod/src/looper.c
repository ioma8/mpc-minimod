/* Live looper engine.
 *
 * Records pad loops from the firmware's audio input stream (the source the
 * sampler itself hears) and mixes them into the firmware's audio output. Both
 * hooks run on audio threads, so nothing here allocates, locks or touches the
 * app's model: the loop pool is mapped once when the view opens and every
 * cross-thread field is published with release/acquire atomics.
 *
 * The output side is the clock: every played frame advances one transport
 * counter, and a loop is an exact count of those frames. That keeps playback
 * and recording in the same time base even when the two audio streams drift.
 *
 * Latency: the firmware keeps a full ALSA ring in each direction, so an input
 * frame read now was captured one input ring ago and an output frame written
 * now is heard one output ring later. The write position is shifted back by
 * that total; the first frames of a take are dropped (they belong before the
 * loop start) and the closing press is followed by a short drain so the last
 * ring of the performance is not lost either.
 */
#include <math.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#include "looper.h"

#define LP_RATE 44100u
#define LP_SECONDS 8u
#define LP_MAX_FRAMES (LP_RATE * LP_SECONDS)
#define LP_RING 32768u
#define LP_BLOCK 512u
#define LP_GAIN 0.85f
#define LP_CLICK 420u
#define LP_LAT_MAX 16384u

static float *g_pool; /* LOOPER_PADS * LP_MAX_FRAMES mono floats */
static volatile int g_open;

static volatile int g_click; /* metronome click on the sync pulse */
static volatile int g_sync;
static volatile unsigned g_sync_frames;
static volatile uint64_t g_pulse_epoch;
static volatile uint64_t g_t;    /* played frames since open */
static volatile unsigned g_lat;  /* record latency, frames */
static volatile unsigned g_master = 127; /* MAIN VOLUME, 0..127 */

static volatile unsigned g_lat_used; /* latency snapshotted for the last take */
static volatile char g_lat_src = 'E'; /* E estimate, B ring geometry, F fixed */

/* offsets: state 0, len 4, rec 8, lat 12, origin 16, skip 24, touched 32,
 * drain 36; stride 40. The emulator test reads this array through
 * g_pad_stride, so keep that symbol in sync with the struct. */
struct pad {
    volatile int state;
    volatile unsigned len;   /* closed length, frames */
    volatile unsigned rec;   /* frames captured, writer: audio thread only */
    volatile unsigned lat;   /* latency applied to this take */
    volatile uint64_t origin;
    volatile unsigned skip;  /* frames still to drop at the start of a take */
    volatile int touched;    /* during DRAIN: 1 when the take was an overdub */
    volatile unsigned drain; /* DRAIN: frames still to capture after the press */
};
volatile unsigned g_pad_stride = sizeof(struct pad);

static struct pad g_pads[LOOPER_PADS];

/* mono input ring: input thread writes, output thread reads */
static float g_ring[LP_RING];
static volatile unsigned g_ring_w;
static volatile unsigned g_ring_r;

static volatile int g_rec_count; /* pads in REC, DUB or DRAIN */
static volatile unsigned g_input_level; /* float bits */

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

static void drop_rec(void)
{
    long v = __atomic_sub_fetch(&g_rec_count, 1, __ATOMIC_ACQ_REL);
    if (v < 0)
        __atomic_store_n(&g_rec_count, 0, __ATOMIC_RELEASE);
}

/* Give a finished take its musical length: the transport span, snapped to a
 * whole number of pulses while the sync pulse is running. The loop starts at
 * the press, not at a pulse: moving the origin would shift the performance
 * itself, which is worse than starting off the beat. */
static void close_length(struct pad *p)
{
    unsigned span = (unsigned)(g_t - p->origin);
    unsigned len = quantize(span ? span : p->rec);
    p->len = len ? len : p->rec;
}

/* One thread wins the transition out of REC/DUB; the loser returns without
 * touching the recording count. */
static void finish_pad(struct pad *p)
{
    int st = __atomic_load_n(&p->state, __ATOMIC_ACQUIRE);
    if (st != LOOP_REC && st != LOOP_DUB)
        return;
    int expected = st;
    int next = p->lat ? LOOP_DRAIN : LOOP_PLAY;
    if (!__atomic_compare_exchange_n(&p->state, &expected, next, 0,
                                     __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE))
        return;
    if (st == LOOP_REC)
        close_length(p);
    p->touched = (st == LOOP_DUB);
    p->drain = p->lat;
    p->skip = 0;
    if (!p->drain)
        drop_rec();
}

void looper_stop_all(void)
{
    for (int i = 0; i < LOOPER_PADS; ++i) {
        struct pad *p = &g_pads[i];
        int st = __atomic_load_n(&p->state, __ATOMIC_ACQUIRE);
        if (st == LOOP_REC || st == LOOP_DUB || st == LOOP_DRAIN) {
            int expected = st;
            if (__atomic_compare_exchange_n(&p->state, &expected, LOOP_STOP, 0,
                                            __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
                if (st == LOOP_REC)
                    close_length(p);
                p->drain = 0;
                p->skip = 0;
                drop_rec();
            }
        } else if (st == LOOP_PLAY) {
            __atomic_store_n(&p->state, LOOP_STOP, __ATOMIC_RELEASE);
        }
    }
}

static void clear_pad(int pad)
{
    struct pad *p = &g_pads[pad];
    int st = __atomic_load_n(&p->state, __ATOMIC_ACQUIRE);
    if (st == LOOP_REC || st == LOOP_DUB || st == LOOP_DRAIN) {
        int expected = st;
        if (__atomic_compare_exchange_n(&p->state, &expected, LOOP_IDLE, 0,
                                        __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE))
            drop_rec();
    }
    __atomic_store_n(&p->state, LOOP_IDLE, __ATOMIC_RELEASE);
    p->len = 0;
    p->rec = 0;
    p->skip = 0;
    p->drain = 0;
    p->origin = 0;
    p->lat = 0;
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
        int st = g_pads[i].state;
        __atomic_store_n(&g_pads[i].state,
                         g_pads[i].len ? LOOP_STOP : LOOP_IDLE, __ATOMIC_RELEASE);
        (void)st;
        g_pads[i].rec = 0;
        g_pads[i].skip = 0;
        g_pads[i].drain = 0;
        g_pads[i].origin = 0;
        g_pads[i].lat = 0;
        g_pads[i].touched = 0;
    }
    __atomic_store_n(&g_ring_w, 0, __ATOMIC_RELEASE);
    __atomic_store_n(&g_ring_r, 0, __ATOMIC_RELEASE);
    __atomic_store_n(&g_rec_count, 0, __ATOMIC_RELEASE);
    g_input_level = 0;
    g_t = 0;
    g_sync = 0;
    g_sync_frames = 0;
    g_pulse_epoch = 0;
    g_click = 0; /* the metronome is off by default */
    g_msg[0] = 0;
    g_open = 1;
    say("PAD: record   SHIFT+PAD: overdub");
    return 1;
}

void looper_close(void)
{
    if (!g_open)
        return;
    looper_stop_all();
    g_sync = 0;
    g_input_level = 0;
    g_open = 0;
}

/* A new take starts from the newest input frame: anything older in the ring
 * belongs to the previous take and would play as a ghost at the start. */
static void flush_input(void)
{
    unsigned w = __atomic_load_n(&g_ring_w, __ATOMIC_ACQUIRE);
    __atomic_store_n(&g_ring_r, w, __ATOMIC_RELEASE);
}

static void start_record(struct pad *p, int pad)
{
    float *buf = pad_buf(pad);
    if (!buf)
        return;
    /* Silence the previous take before the audio thread can see the new state,
     * and touch the pages on this (non-audio) thread. */
    __atomic_store_n(&p->state, LOOP_IDLE, __ATOMIC_RELEASE);
    memset(buf, 0, (size_t)LP_MAX_FRAMES * sizeof(float));
    p->len = 0;
    p->rec = 0;
    p->skip = 0;
    p->drain = 0;
    p->touched = 0;
    p->origin = g_t;
    p->lat = g_lat > LP_LAT_MAX ? LP_LAT_MAX : g_lat;
    g_lat_used = p->lat;
    p->skip = p->lat;
    flush_input();
    __atomic_add_fetch(&g_rec_count, 1, __ATOMIC_ACQ_REL);
    __atomic_store_n(&p->state, LOOP_REC, __ATOMIC_RELEASE);
}

static void start_overdub(struct pad *p)
{
    if (!p->len)
        return;
    p->lat = g_lat > LP_LAT_MAX ? LP_LAT_MAX : g_lat;
    g_lat_used = p->lat;
    p->drain = 0;
    p->skip = 0;
    flush_input();
    __atomic_add_fetch(&g_rec_count, 1, __ATOMIC_ACQ_REL);
    __atomic_store_n(&p->state, LOOP_DUB, __ATOMIC_RELEASE);
}

int looper_pad(int pad, int shift, int erase)
{
    if (!g_open)
        return 0;
    if (pad < 0 || pad >= LOOPER_PADS)
        return 1;
    struct pad *p = &g_pads[pad];
    if (erase) {
        clear_pad(pad);
        say("PAD CLEARED");
        return 1;
    }
    int st = __atomic_load_n(&p->state, __ATOMIC_ACQUIRE);
    if (shift) {
        if (st == LOOP_PLAY || st == LOOP_STOP)
            start_overdub(p);
        else if (st == LOOP_DUB || st == LOOP_REC || st == LOOP_DRAIN)
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
    case LOOP_DUB:
        finish_pad(p);
        break;
    case LOOP_PLAY:
        __atomic_store_n(&p->state, LOOP_STOP, __ATOMIC_RELEASE);
        break;
    case LOOP_STOP:
        p->origin = g_t;
        __atomic_store_n(&p->state, LOOP_PLAY, __ATOMIC_RELEASE);
        break;
    case LOOP_DRAIN:
        break; /* the take is already closing */
    }
    return 1;
}

int looper_button(unsigned code, int down, int shift, int erase)
{
    if (!g_open)
        return 0;
    if (code == 0x6f) { /* DATA: SHIFT+DATA closes */
        if (down && shift)
            looper_close();
        return 1;
    }
    if (code == 0x51) { /* STOP: stop every loop, keep the material */
        if (down) {
            looper_stop_all();
            say("ALL STOPPED");
        }
        return 1;
    }
    if (code == 0x4a) { /* LOOP: sync pulse; ERASE+LOOP clears all; SHIFT+LOOP click */
        if (down) {
            if (erase) {
                for (int i = 0; i < LOOPER_PADS; ++i)
                    clear_pad(i);
                g_sync = 0;
                g_sync_frames = 0;
                say("ALL LOOPS CLEARED");
            } else if (shift) {
                g_click = !g_click;
                say(g_click ? "CLICK on" : "CLICK off");
            } else if (g_sync) {
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

void looper_set_master(unsigned value)
{
    g_master = value > 127 ? 127 : value;
}

unsigned looper_master(void)
{
    return g_master;
}

void looper_set_latency_ex(unsigned frames, char source)
{
    g_lat = frames > LP_LAT_MAX ? LP_LAT_MAX : frames;
    g_lat_src = source;
}

unsigned looper_input_backlog(void)
{
    unsigned w = __atomic_load_n(&g_ring_w, __ATOMIC_ACQUIRE);
    unsigned r = __atomic_load_n(&g_ring_r, __ATOMIC_ACQUIRE);
    return w - r;
}

void looper_notice(const char *text)
{
    say(text);
}

/* ------------------------------------------------------------ audio input */
/* Append one mono block to the ring. Only the selected input stream feeds it
 * (the ALSA interposer gates on the pcm handle), so this is single-producer. */
static void input_block(const float *mono, unsigned n, float peak)
{
    int rec = __atomic_load_n(&g_rec_count, __ATOMIC_ACQUIRE);
    if (rec) {
        unsigned w = g_ring_w;
        for (unsigned i = 0; i < n; ++i) {
            unsigned r = __atomic_load_n(&g_ring_r, __ATOMIC_RELAXED);
            if (w + 1 - r < LP_RING) {
                g_ring[w % LP_RING] = mono[i];
                w++;
            } else {
                break;
            }
        }
        __atomic_store_n(&g_ring_w, w, __ATOMIC_RELEASE);
    }
    /* peak with a decay, so the meter falls back after the source stops */
    float prev;
    unsigned pb = g_input_level;
    memcpy(&prev, &pb, sizeof prev);
    float level = prev * 0.9f;
    if (peak > level)
        level = peak;
    if (level > 1.f)
        level = 1.f;
    unsigned nb;
    memcpy(&nb, &level, sizeof nb);
    __atomic_store_n(&g_input_level, nb, __ATOMIC_RELAXED);
}

/* Convert `frames` mono frames of one planar S32 channel and feed the ring in
 * LP_BLOCK chunks, so a large read is never truncated. */
void looper_input_s32(const int32_t *const *channels, int count, unsigned frames)
{
    if (!g_open || !channels || count < 1 || !channels[0] || !frames)
        return;
    const int32_t *l = channels[0];
    for (unsigned off = 0; off < frames; off += LP_BLOCK) {
        unsigned n = frames - off > LP_BLOCK ? LP_BLOCK : frames - off;
        float tmp[LP_BLOCK];
        float peak = 0.f;
        for (unsigned i = 0; i < n; ++i) {
            float v = (float)l[off + i] * (1.0f / 2147483648.0f);
            float a = v < 0.f ? -v : v;
            if (a > peak)
                peak = a;
            tmp[i] = v;
        }
        input_block(tmp, n, peak);
    }
}

/* Interleaved S32 capture (snd_pcm_readi): channels 0/1 carry the source. */
void looper_input_s32_interleaved(const int32_t *buf, unsigned channels,
                                  unsigned frames)
{
    if (!g_open || !buf || !channels || !frames)
        return;
    for (unsigned off = 0; off < frames; off += LP_BLOCK) {
        unsigned n = frames - off > LP_BLOCK ? LP_BLOCK : frames - off;
        float tmp[LP_BLOCK];
        float peak = 0.f;
        for (unsigned i = 0; i < n; ++i) {
            const int32_t *f = buf + (size_t)(off + i) * channels;
            float v = channels >= 2 ? ((float)f[0] + (float)f[1]) *
                                          (0.5f / 2147483648.0f)
                                    : (float)f[0] * (1.0f / 2147483648.0f);
            float a = v < 0.f ? -v : v;
            if (a > peak)
                peak = a;
            tmp[i] = v;
        }
        input_block(tmp, n, peak);
    }
}

/* ----------------------------------------------------------- audio output */
static float click_sample(uint64_t phase)
{
    if (phase >= LP_CLICK)
        return 0.f;
    float env = expf(-(float)phase / 70.f);
    return sinf(6.2831853f * 1000.f * (float)phase / (float)LP_RATE) * env * 0.35f;
}

/* Mix one sample into a loop (overdub position, latency shifted, wrapped). */
static void dub_add(float *b, unsigned len, unsigned lat, uint64_t pos, float v)
{
    int64_t at = (int64_t)(pos % len) - (int64_t)lat;
    if (at < 0)
        at += len;
    if (at < 0 || (unsigned)at >= len)
        return; /* lp_lat < len, so this is unreachable; kept as a guard */
    float sum = b[at] + v;
    if (sum > 1.f)
        sum = 1.f;
    else if (sum < -1.f)
        sum = -1.f;
    b[at] = sum;
}

/* Mix the loops into the interleaved S32 block the firmware is about to hand to
 * the device. This is the one point every audio path goes through, so it does
 * not matter which device object renders the app's own sound. */
void looper_render_s32(int32_t *buf, unsigned channels, unsigned frames)
{
    if (!g_open || !buf || !channels || !frames)
        return;
    int rec = __atomic_load_n(&g_rec_count, __ATOMIC_ACQUIRE);
    unsigned done = 0;
    while (done < frames) {
        unsigned n = frames - done;
        if (n > LP_BLOCK)
            n = LP_BLOCK;
        float in[LP_BLOCK];
        float add[LP_BLOCK];
        unsigned got = 0;
        if (rec) {
            unsigned r = __atomic_load_n(&g_ring_r, __ATOMIC_ACQUIRE);
            unsigned w = __atomic_load_n(&g_ring_w, __ATOMIC_ACQUIRE);
            unsigned avail = w - r;
            if (avail > n * 4) {
                r += avail - n * 2; /* keep the ring shallow */
                avail = n * 2;
            }
            got = avail < n ? avail : n;
            for (unsigned i = 0; i < got; ++i)
                in[i] = g_ring[(r + i) % LP_RING];
            __atomic_store_n(&g_ring_r, r + got, __ATOMIC_RELEASE);
        }
        for (unsigned i = got; i < n; ++i)
            in[i] = 0.f;
        for (unsigned i = 0; i < n; ++i)
            add[i] = 0.f;

        uint64_t t = g_t;
        for (int p = 0; p < LOOPER_PADS; ++p) {
            struct pad *pd = &g_pads[p];
            int st = __atomic_load_n(&pd->state, __ATOMIC_ACQUIRE);
            if (st != LOOP_PLAY && st != LOOP_DUB && st != LOOP_DRAIN &&
                st != LOOP_REC)
                continue;
            unsigned len = pd->len;
            float *b = pad_buf(p);
            if (!b)
                continue;
            int play = st == LOOP_PLAY || st == LOOP_DUB || st == LOOP_DRAIN;
            if (play && len) {
                uint64_t base = t - pd->origin;
                for (unsigned i = 0; i < n; ++i)
                    add[i] += b[(base + i) % len] * LP_GAIN;
            }
            if (st == LOOP_DUB || (st == LOOP_DRAIN && pd->touched)) {
                unsigned lat = len ? pd->lat % len : 0;
                for (unsigned i = 0; i < n; ++i)
                    dub_add(b, len, lat, t + i - pd->origin, in[i]);
            } else if (st == LOOP_REC || (st == LOOP_DRAIN && !pd->touched)) {
                unsigned rn = pd->rec;
                unsigned skip = pd->skip;
                for (unsigned i = 0; i < n; ++i) {
                    if (skip) {
                        --skip;
                        continue;
                    }
                    if (rn >= LP_MAX_FRAMES)
                        break;
                    b[rn++] = in[i];
                }
                pd->skip = skip;
                pd->rec = rn;
            }
            if (st == LOOP_DRAIN) {
                unsigned d = pd->drain;
                d = d > n ? d - n : 0;
                pd->drain = d;
                if (!d) {
                    pd->skip = 0;
                    __atomic_store_n(&pd->state, LOOP_PLAY, __ATOMIC_RELEASE);
                    drop_rec();
                }
            }
        }
        if (g_sync && g_click) {
            unsigned q = g_sync_frames;
            if (q) {
                uint64_t e = g_pulse_epoch;
                for (unsigned i = 0; i < n; ++i)
                    add[i] += click_sample((t + i - e) % q);
            }
        }
        float peak = 0.f;
        float gain = (float)g_master * (1.0f / 127.0f);
        for (unsigned i = 0; i < n; ++i) {
            int32_t *frame = buf + (size_t)(done + i) * channels;
            float a = add[i] * gain;
            for (unsigned c = 0; c < channels; ++c) {
                float v = (float)frame[c] * (1.0f / 2147483648.0f) + a;
                if (v > 1.f)
                    v = 1.f;
                else if (v < -1.f)
                    v = -1.f;
                float m = v < 0.f ? -v : v;
                if (m > peak)
                    peak = m;
                frame[c] = (int32_t)(v * 2147483647.0f);
            }
        }
        (void)peak;
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
    view->click = g_click;
    view->sync_frames = g_sync_frames;
    view->pulse_phase = 0xffffffffu;
    if (g_sync && g_sync_frames)
        view->pulse_phase = (unsigned)((g_t - g_pulse_epoch) % g_sync_frames);
    unsigned lb = g_input_level;
    float lv;
    memcpy(&lv, &lb, sizeof lv);
    view->input = lv > 1.f ? 1.f : lv;
    view->recording_pad = -1;
    view->lat_frames = g_lat_used;
    view->lat_src = g_lat_src;
    uint64_t t = g_t;
    for (int i = 0; i < LOOPER_PADS; ++i) {
        struct pad *p = &g_pads[i];
        struct looper_pad_view *v = &view->pads[i];
        int st = __atomic_load_n(&p->state, __ATOMIC_ACQUIRE);
        /* a draining take is still a take to the user */
        if (st == LOOP_DRAIN)
            st = p->touched ? LOOP_DUB : LOOP_REC;
        v->state = (unsigned char)st;
        v->len_frames = p->len;
        v->rec_frames = p->rec;
        if ((st == LOOP_PLAY || st == LOOP_DUB) && p->len)
            v->pos = (unsigned)((t - p->origin) % p->len);
        if (st == LOOP_REC || st == LOOP_DUB)
            if (view->recording_pad < 0)
                view->recording_pad = i;
    }
    if (g_msg[0] && now_ms() < g_msg_until)
        memcpy(view->message, g_msg, sizeof view->message - 1);
    return 1;
}
