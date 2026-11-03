/* mpcmini — a minimal LD_PRELOAD mod for the Akai MPC Sample (firmware 1.3.0.12).
 *
 * It never touches the firmware on disk: the SD autorun script preloads this
 * library into the stock app, we find the app's load base and install a few
 * in-memory branch hooks (the same technique am-mod documents).
 *
 * Features
 *   1. octave shift in 16 Levels: PLUS / MINUS move the whole chromatic row by
 *      an octave while 16 LEVELS is lit.
 *   2. chromatic control from an external MIDI controller (always on).
 *   3. autotrim of the sample shown on the Sample page: SHIFT + B3 trims Start
 *      and End to the silent edges.
 *
 * Every hook offset below is absolute for 1.3.0.12 only. The loader refuses to
 * install anything when the binary does not match, so a different firmware just
 * means "no mod" instead of "crash".
 */
#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <elf.h>
#include <errno.h>
#include <fcntl.h>
#include <link.h>
#include <math.h>
#include <pthread.h>
#include <signal.h>
#include <ucontext.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <time.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>

/* ---------------------------------------------------------------- anchors */
/* application file offsets (== virtual addresses in the PIE of 1.3.0.12) */
#define A_SIXTEEN_CTOR 0x1545a50   /* 16 Levels screen constructor        */
#define A_SIXTEEN_PROC 0x21472f0   /* 16 Levels pitch computation         */
#define A_BUTTONS 0x159f2f0        /* physical button dispatch (event)    */
#define A_NOTE_ON 0x1e27360        /* note on (input, src, pad, props, v) */
#define A_NOTE_OFF 0x1e270d0       /* note off (input, src, pad)           */
#define A_CS_EVENT 0x23aff60       /* control-surface event (self, c, p)  */
#define A_TRIM_WRITE 0x14d0890     /* generic observable property setter  */
#define A_SCREEN_RESIZED 0x1660370 /* screen resize notification          */
#define A_SAMPLE_FRAMES 0x2941f80  /* sample frame count getter           */
#define A_PROG_INST 0x286bbe0      /* program -> instrument accessor      */
#define A_ROTARY 0x15f5690         /* rotary connection core              */
#define A_REPAINT 0x1149880        /* component repaint (comp, x, y)      */
#define A_TRIM_CORE 0x15e70c0      /* trim core (float value, param)      */
#define A_BROADCAST 0x13bb930      /* value broadcaster (property, float) */
#define A_TRIM_REG 0x16b6060       /* waveform locator bounds (probe)     */
#define A_STR_ASSIGN 0xf535c0      /* juce::String assign (dest, text)    */
#define A_OBS_SET 0x14a7580        /* observable setter (float, property) */
#define A_LABEL_TEXT 0x11bae00     /* Label::getText (returns String&)    */

#define T_LAYER 0xb0 /* -> context; [ctx+0x60] = instrument */
#define T_MODEL 0x70
#define T_VALUE 0x19c /* float, normalised 0..1 */
#define T_APPLIED 0x1a0 /* float, the value the engine applied */
#define T_MIN 0x1e0   /* u32, frames */
#define T_MAX 0x1e8   /* u32, frames */

#define A_TRIM_PARAM_VT 0x6e1b240 /* TrimParameter vtable */
#define A_TRIM_PAGE_VT 0x6e31ee8  /* the Trim page class */
#define A_LOC_BOUNDS 0x16b6060     /* AC50WaveformLocator::bounds */
#define A_PITCH_OFF 0x27e4400      /* 16 Levels: set pitch offset (a3 = semitones) */

#define FIELD(p, off, type) (*(type *)((char *)(p) + (off)))
#define FN(off, type) ((type)(base + (off)))

/* ---------------------------------------------------------------- state */
static int log_fd = -1;
static uintptr_t base;
static unsigned char *relay_page;
static unsigned char *tramp_page;
static int nslots, ntramp;
static int relay_used;
static int hook_mask = 0xf2; /* 2 proc, 16 buttons, 32 note_on, 64 cs, 128 note_off */ /* 1 ctor (crashes, disabled), 2 proc, 16 buttons,
                                32 note_on, 64 cs_event, 128 note_off */ /* 1 ctor, 2 proc, 4 plus, 8 minus, 16 buttons,
                                32 note_on, 64 cs_event, 128 note_off */
static int safe_mode;
static int debug_mode;
static int probe_mode;
static int probe_bits = 0x7f;

static void mlog(const char *fmt, ...);
static void *g_screen;
static void *g_trim_page;
static int prop_references(uintptr_t P, uintptr_t param);
static int find_screens(uintptr_t *out, int max);

static uintptr_t g_watch_page;
#define MAXW 12
static uintptr_t g_wpages[MAXW];
static int g_nwpage;

static int watch_has(uintptr_t page)
{
    for (int i = 0; i < g_nwpage; ++i)
        if (g_wpages[i] == page)
            return 1;
    return 0;
}

static void watch_arm(uintptr_t addr, const char *why)
{
    uintptr_t page = addr & ~(uintptr_t)0xfff;
    if (watch_has(page) || g_nwpage >= MAXW)
        return;
    int rc = mprotect((void *)page, 0x1000, PROT_READ);
    if (rc == 0) {
        g_wpages[g_nwpage++] = page;
        mlog("mpcmini: watch(%s) %p page=%p\n", why, (void *)addr, (void *)page);
    } else {
        mlog("mpcmini: watch(%s) %p page=%p failed: %s\n", why, (void *)addr,
             (void *)page, strerror(errno));
    }
}


static void fault_handler(int sig, siginfo_t *si, void *ucv)
{
    ucontext_t *u = (ucontext_t *)ucv;
    unsigned long pc = 0, lr = 0, sp = 0;
    if (u) {
        pc = (unsigned long)u->uc_mcontext.pc;
        lr = (unsigned long)u->uc_mcontext.regs[30];
        sp = (unsigned long)u->uc_mcontext.sp;
    }
    if (si && watch_has((uintptr_t)si->si_addr & ~(uintptr_t)0xfff)) {
        mlog("mpcmini: WATCH addr=%p pc=%p lr=%p sp=%p\n", si->si_addr, (void *)pc,
             (void *)lr, (void *)sp);
        mprotect((void *)((uintptr_t)si->si_addr & ~(uintptr_t)0xfff), 0x1000,
                 PROT_READ | PROT_WRITE);
        for (int i = 0; i < g_nwpage; ++i)
            if (g_wpages[i] == ((uintptr_t)si->si_addr & ~(uintptr_t)0xfff))
                g_wpages[i] = 0;
        return;
    }
    mlog("mpcmini: FAULT sig=%d addr=%p pc=%p lr=%p sp=%p\n", sig,
         si ? si->si_addr : 0, (void *)pc, (void *)lr, (void *)sp);
    _exit(139);
}

static void mlog(const char *fmt, ...)
{
    if (log_fd < 0)
        return;
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    if (n > 0)
        (void)!write(log_fd, buf, (size_t)n);
}

/* ------------------------------------------------------- process memory */
struct rng {
    uintptr_t lo, hi;
    int w;
};
static struct rng g_rng[512];
static int g_nrng;
static int g_rng_loaded;

/* /proc/self/mem keeps reads and writes out of the C compiler's way: no aliasing
 * surprises, no faults when the app unmaps a page under us. */
static int rd(uintptr_t addr, void *dst, size_t n)
{
    int fd = open("/proc/self/mem", O_RDONLY);
    if (fd < 0)
        return 0;
    ssize_t got = pread(fd, dst, n, (off_t)addr);
    close(fd);
    return got == (ssize_t)n;
}

static int wr(uintptr_t addr, const void *src, size_t n)
{
    int fd = open("/proc/self/mem", O_WRONLY);
    if (fd < 0)
        return 0;
    ssize_t put = pwrite(fd, src, n, (off_t)addr);
    close(fd);
    return put == (ssize_t)n;
}

static int mapped(const void *p)
{
    if (!g_rng_loaded)
        return 1;
    uintptr_t a = (uintptr_t)p;
    for (int i = 0; i < g_nrng; ++i)
        if (a >= g_rng[i].lo && a < g_rng[i].hi)
            return 1;
    return 0;
}

static void ranges_load(void)
{
    if (g_rng_loaded)
        return;
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f)
        return;
    char line[512];
    while (fgets(line, sizeof line, f) && g_nrng < 512) {
        unsigned long lo = 0, hi = 0;
        char perms[8] = {0};
        if (sscanf(line, "%lx-%lx %7s", &lo, &hi, perms) != 3)
            continue;
        g_rng[g_nrng].lo = lo;
        g_rng[g_nrng].hi = hi;
        g_rng[g_nrng].w = perms[1] == 'w';
        g_nrng++;
    }
    fclose(f);
    g_rng_loaded = 1;
}

/* ------------------------------------------------------------ hook engine */
/* A site is patched at entry+4 (the instruction after `paciasp`/`bti c`):
 *   entry+4 : b slot            (slot lives in a page mapped near the app)
 *   slot    : ldr x16, #8 ; br x16 ; .quad hook
 *   tramp   : <displaced insn> ; b entry+8
 * The trampoline is what the hook calls to run the original code. */
static uint32_t enc_b(uintptr_t from, uintptr_t to)
{
    long off = (long)to - (long)from;
    if (off < -(1L << 27) || off >= (1L << 27))
        return 0;
    return 0x14000000u | ((uint32_t)(off >> 2) & 0x03FFFFFFu);
}

static int pc_relative(uint32_t insn)
{
    if ((insn & 0x7C000000u) == 0x14000000u) /* B / BL   */
        return 1;
    if ((insn & 0x9F000000u) == 0x90000000u) /* ADRP     */
        return 1;
    if ((insn & 0x9F000000u) == 0x10000000u) /* ADR      */
        return 1;
    if ((insn & 0x3B000000u) == 0x18000000u) /* LDR lit  */
        return 1;
    return 0;
}

/* Each hook saves the caller's stack pointer in the relay slot and the
 * trampoline restores it before running the displaced instruction, so the
 * original function continues on its own frame. */
/* The hook is entered instead of the function, and calls the trampoline to run
 * the original. The trampoline holds a copy of the function's prologue (so the
 * body runs on its own frame) and then branches into the body; the body's
 * epilogue returns straight to the hook, which returns to the app with all
 * callee-saved registers intact. */
static void *install_hook(uintptr_t site, void *hook)
{
    if (!relay_page || !tramp_page || nslots >= 60 || ntramp >= 60)
        return 0;
    uint32_t first = 0;
    if (!rd(site, &first, 4))
        return 0;
    if (first != 0xD503233Fu && first != 0xD503245Fu) {
        mlog("mpcmini: site %p has no paciasp/bti (%08x)\n", (void *)site, first);
        return 0;
    }
    /* Copy as much of the prologue as is safe: everything up to the first
     * pc-relative instruction (those compute their address from their own
     * location, so they must run in place) but never fewer than the erased
     * first instruction. */
    uint32_t pro[4];
    if (!rd(site, pro, sizeof pro))
        return 0;
    unsigned n = 4;
    for (unsigned i = 1; i < 4; ++i)
        if (pc_relative(pro[i])) {
            n = i;
            break;
        }
    unsigned char *slot = relay_page + (size_t)nslots * 32;
    unsigned char *tr = tramp_page + (size_t)ntramp * 32;
    uint32_t b_body = enc_b((uintptr_t)tr + n * 4, site + n * 4);
    uint32_t b_slot = enc_b(site, (uintptr_t)slot);
    if (!b_body || !b_slot) {
        mlog("mpcmini: hook %p out of B range\n", (void *)site);
        return 0;
    }
    uint32_t ldr = 0x58000050u; /* ldr x16, [pc, #8] */
    uint32_t br = 0xD61F0200u;  /* br  x16         */
    memcpy(slot, &ldr, 4);
    memcpy(slot + 4, &br, 4);
    memcpy(slot + 8, &hook, 8);
    memcpy(tr, pro, n * 4);
    memcpy(tr + n * 4, &b_body, 4);
    nslots++;
    ntramp++;

    uintptr_t page = site & ~(uintptr_t)0xfff;
    if (mprotect((void *)page, 0x2000, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) {
        mlog("mpcmini: mprotect(%p) failed: %s\n", (void *)page, strerror(errno));
        return 0;
    }
    int ok = wr(site, &b_slot, 4);
    __builtin___clear_cache((char *)site, (char *)(site + 4));
    mprotect((void *)page, 0x2000, PROT_READ | PROT_EXEC);
    if (!ok) {
        mlog("mpcmini: patch of %p failed\n", (void *)site);
        return 0;
    }
    __builtin___clear_cache((char *)tr, (char *)tr + n * 4 + 4);
    __builtin___clear_cache((char *)slot, (char *)slot + 16);
    relay_used = 1;
    mlog("mpcmini: hook %p (prologue %u)\n", (void *)site, n);
    return tr;
}

/* -------------------------------------------------------- 16 Levels state */
static int g_octave;
static int g_plus_held, g_minus_held;   /* octave shift applied while 16 LEVELS is active */
static void *g_sixteen; /* the live 16 Levels screen, if seen             */

static int sixteen_tune_active(void)
{
    return g_octave != 0;
}

static int bump_octave(int d)
{
    mlog("mpcmini: PLUS/MINUS -> %+d x 8 semitones\n", d);
    int next = g_octave + d;
    if (next < -4)
        next = -4;
    if (next > 4)
        next = 4;
    if (next == g_octave)
        return 0;
    g_octave = next;
    mlog("mpcmini: pitch shift=%d (%+d semitones)\n", next, next * 8);
    return 1;
}

/* ------------------------------------------------------------- WAV helpers */
static int wav_info(const char *path, unsigned *frames, unsigned *rate,
                    unsigned *ch, unsigned *dataoff, unsigned *bps)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    unsigned char h[4096];
    ssize_t n = pread(fd, h, sizeof h, 0);
    if (n < 44 || memcmp(h, "RIFF", 4) || memcmp(h + 8, "WAVE", 4)) {
        close(fd);
        return 0;
    }
    unsigned chn = (unsigned)h[22] | ((unsigned)h[23] << 8);
    unsigned bits = (unsigned)h[34] | ((unsigned)h[35] << 8);
    unsigned rt = (unsigned)h[24] | ((unsigned)h[25] << 8) |
                  ((unsigned)h[26] << 16) | ((unsigned)h[27] << 24);
    if (!chn || !rt || (bits != 16 && bits != 24 && bits != 32)) {
        close(fd);
        return 0;
    }
    /* Walk the chunk list properly: Akai files carry a large metadata chunk
     * before "data", so a fixed-size prefix scan is not enough. */
    unsigned dof = 0, len = 0;
    off_t off = 12;
    for (int guard = 0; guard < 64; ++guard) {
        unsigned char ck[8];
        if (pread(fd, ck, 8, off) != 8)
            break;
        unsigned sz = (unsigned)ck[4] | ((unsigned)ck[5] << 8) |
                      ((unsigned)ck[6] << 16) | ((unsigned)ck[7] << 24);
        if (!memcmp(ck, "data", 4)) {
            dof = (unsigned)off + 8;
            len = sz;
            break;
        }
        off += 8 + (off_t)sz + (off_t)(sz & 1);
    }
    close(fd);
    if (!dof || !len)
        return 0;
    *frames = len / ((bits / 8u) * chn);
    *rate = rt;
    *ch = chn;
    *dataoff = dof;
    *bps = bits / 8u;
    return 1;
}

static short pcm_at(const unsigned char *p, unsigned bps)
{
    if (bps == 2) {
        short v;
        memcpy(&v, p, 2);
        return v;
    }
    if (bps == 3) {
        int v = (int)p[0] | ((int)p[1] << 8) | ((int)(signed char)p[2] << 16);
        return (short)(v >> 8);
    }
    int v;
    memcpy(&v, p, 4);
    return (short)(v >> 16);
}

static int wav_edges(const char *path, unsigned frames, unsigned dataoff,
                     unsigned ch, unsigned bps, unsigned *first, unsigned *last)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    if (!ch)
        ch = 1;
    if (bps < 2 || bps > 4)
        bps = 2;
    /* Threshold: relative to the file's peak so quiet material is trimmed as
     * well, with an absolute floor so a nearly silent file is not trimmed to
     * nothing. MPCMINI_TRIM_DB overrides the relative part (default -50 dB). */
    unsigned char buf[16384];
    short thr = 328; /* -40 dBFS fallback */
    {
        double db = -40.0;
        const char *e = getenv("MPCMINI_TRIM_DB");
        if (e && *e)
            db = atof(e);
        if (db < -90.0) db = -90.0;
        if (db > -6.0) db = -6.0;
        double ratio = pow(10.0, db / 20.0);
        short peak = 0;
        unsigned g = 0;
        while (g < frames) {
            ssize_t n = pread(fd, buf, sizeof buf, (off_t)dataoff + (off_t)g * bps * ch);
            if (n <= 0)
                break;
            unsigned got = (unsigned)(n / (bps * ch));
            for (unsigned i = 0; i < got; ++i)
                for (unsigned c = 0; c < ch; ++c) {
                    short v = pcm_at(buf + (i * ch + c) * bps, bps);
                    if (v < 0) v = (short)-v;
                    if (v > peak) peak = v;
                }
            g += got;
            if (got < sizeof buf / (bps * ch))
                break;
        }
        if (peak > 0) {
            double t = (double)peak * ratio;
            if (t < 90.0) t = 90.0;      /* absolute floor ~ -51 dBFS */
            if (t > 30000.0) t = 30000.0;
            thr = (short)t;
            mlog("mpcmini: trim threshold ratio %.4f peak %d -> %d\n", ratio,
                 (int)peak, (int)thr);
            if (getenv("MPCMINI_ENV")) {
                /* level envelope in 32 windows, same scale as the threshold */
                unsigned step = frames / 32 ? frames / 32 : 1;
                for (unsigned w = 0; w < 32; ++w) {
                    unsigned a = w * step, b = a + step;
                    if (b > frames)
                        b = frames;
                    short wp = 0;
                    for (unsigned f = a; f < b; f += 1) {
                        unsigned char sb[8];
                        if (!rd(0, sb, 0))
                            break;
                        /* read one frame */
                        if (pread(fd, sb, (size_t)(bps * ch), (off_t)dataoff + (off_t)f * bps * ch) <= 0)
                            break;
                        for (unsigned c = 0; c < ch; ++c) {
                            short v = pcm_at(sb + c * bps, bps);
                            if (v < 0) v = (short)-v;
                            if (v > wp) wp = v;
                        }
                    }
                    mlog("mpcmini: env %2u/%02u peak %5d\n", w, 32, (int)wp);
                }
            }
        }
    }
    unsigned f = 0;
    int found_first = 0;
    while (f < frames && !found_first) {
        ssize_t n = pread(fd, buf, sizeof buf, (off_t)dataoff + (off_t)f * bps * ch);
        if (n <= 0)
            break;
        unsigned got = (unsigned)(n / (bps * ch));
        for (unsigned i = 0; i < got; ++i) {
            int loud = 0;
            for (unsigned c = 0; c < ch; ++c) {
                short v = pcm_at(buf + (i * ch + c) * bps, bps);
                if (v > thr || v < -thr)
                    loud = 1;
            }
            if (loud) {
                *first = f + i;
                found_first = 1;
                break;
            }
        }
        f += got;
        if (got < sizeof buf / (bps * ch))
            break;
    }
    if (!found_first) {
        close(fd);
        return 0;
    }
    unsigned stop = f > frames ? frames : f;
    unsigned lastf = *first;
    /* Walk backwards in blocks: one pread per frame froze the UI thread on a
     * 15k-frame sample, and the stock library has 500k-frame ones. */
    unsigned block = (unsigned)(sizeof buf / (bps * ch));
    if (!block)
        block = 1;
    unsigned end = frames;
    while (end > 0) {
        unsigned start = end > block ? end - block : 0;
        unsigned cnt = end - start;
        ssize_t n = pread(fd, buf, (size_t)cnt * bps * ch,
                          (off_t)dataoff + (off_t)start * bps * ch);
        if (n <= 0)
            break;
        unsigned got = (unsigned)(n / (bps * ch));
        int found = 0;
        for (unsigned i = got; i-- > 0;) {
            int loud = 0;
            for (unsigned c = 0; c < ch; ++c) {
                short v = pcm_at(buf + (i * ch + c) * bps, bps);
                if (v > thr || v < -thr)
                    loud = 1;
            }
            if (loud) {
                lastf = start + i;
                found = 1;
                break;
            }
        }
        if (found)
            break;
        end = start;
    }
    (void)stop;
    close(fd);
    *last = lastf;
    return 1;
}

/* --------------------------------------------------------- trim parameter */
static struct {
    void *p;
    float val;
    unsigned mn, mx;
    int bound;
} g_tp[256];
static int g_ntp;

static void scan_trim_params(void)
{
    if (g_ntp)
        return;
    ranges_load();
    uintptr_t want = base + A_TRIM_PARAM_VT;
    unsigned char *buf = malloc(65536);
    if (!buf)
        return;
    for (int r = 0; r < g_nrng && g_ntp < 256; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && g_ntp < 256;
             a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && g_ntp < 256; o += 8) {
                uintptr_t v;
                memcpy(&v, buf + o, 8);
                if (v != want)
                    continue;
                uintptr_t p = a + o;
                float val = 0;
                unsigned mn = 0, mx = 0;
                rd(p + T_VALUE, &val, 4);
                rd(p + T_MIN, &mn, 4);
                rd(p + T_MAX, &mx, 4);
                g_tp[g_ntp].p = (void *)p;
                g_tp[g_ntp].val = val;
                g_tp[g_ntp].mn = mn;
                g_tp[g_ntp].mx = mx;
                g_tp[g_ntp].bound = 0;
                g_ntp++;
            }
        }
    }
    free(buf);
    mlog("mpcmini: trimparams found=%d\n", g_ntp);
}

static unsigned frames_of_sample(uintptr_t sample)
{
    uintptr_t meta = 0;
    if (!sample || !rd(sample + 0x130, &meta, 8) || !meta)
        return 0;
    unsigned short ch = 0;
    unsigned fr = 0, rt = 0;
    if (!rd(meta, &ch, 2))
        return 0;
    rd(meta + 4, &fr, 4);
    rd(meta + 8, &rt, 4);
    if (ch != 1 && ch != 2)
        return 0;
    if (rt < 8000 || rt > 192000)
        return 0;
    return fr;
}

static int page_layers(uintptr_t param, uintptr_t *layers, uintptr_t *instr)
{
    uintptr_t ctx = 0;
    if (!rd(param + T_LAYER, &ctx, 8) || !ctx)
        return 0;
    uintptr_t ins = 0;
    if (!rd(ctx + 0x60, &ins, 8) || !ins)
        return 0;
    uintptr_t lay = 0;
    if (!rd(ins + 0x22e8, &lay, 8) || !lay)
        return 0;
    *layers = lay;
    *instr = ins;
    return 1;
}

static int path_from_buffer(const char *buf, unsigned want, char *out, size_t n)
{
    static const char *roots[] = {"/data/", "/media/", "/usr/share/", "/factory/"};
    for (unsigned r = 0; r < 4; ++r) {
        const char *hit = strstr(buf, roots[r]);
        if (!hit)
            continue;
        const char *end = hit;
        while (*end && *end >= 32 && *end < 127)
            end++;
        if (end - hit < 6 || end - hit > 500 || !strcasestr(hit, ".wav"))
            continue;
        char tmp[512];
        snprintf(tmp, sizeof tmp, "%.*s", (int)(end - hit), hit);
        unsigned fr = 0, rt = 0, ch = 0, doff = 0, bps = 2;
        if (!wav_info(tmp, &fr, &rt, &ch, &doff, &bps) || fr != want)
            continue;
        snprintf(out, n, "%s", tmp);
        return 1;
    }
    return 0;
}

static int wav_path_of(uintptr_t obj, unsigned want, char *out, size_t n)
{
    ranges_load();
    char buf[512];
    for (long o = 0; o < 0x2000; o += 8) {
        uintptr_t q = 0;
        if (!rd(obj + o, &q, 8) || !q || !mapped((void *)q))
            continue;
        if (!rd(q, buf, sizeof buf - 1))
            continue;
        buf[sizeof buf - 1] = 0;
        if (path_from_buffer(buf, want, out, n)) {
            mlog("mpcmini: path from sample+0x%lx -> %s\n", (unsigned long)o, out);
            return 1;
        }
        /* one level deeper: the path often lives in a small holder object */
        for (long k = 0; k < 0x400; k += 8) {
            uintptr_t q2 = 0;
            if (!rd(q + k, &q2, 8) || !q2 || !mapped((void *)q2))
                continue;
            if (!rd(q2, buf, sizeof buf - 1))
                continue;
            buf[sizeof buf - 1] = 0;
            if (path_from_buffer(buf, want, out, n)) {
                mlog("mpcmini: path from sample+0x%lx/+0x%lx -> %s\n",
                     (unsigned long)o, (unsigned long)k, out);
                return 1;
            }
        }
    }
    for (long d = -0x2000; d <= 0x2000; d += 8) {
        uintptr_t a = (uintptr_t)((long)obj + d);
        if (!mapped((void *)a) || !rd(a, buf, sizeof buf - 1))
            continue;
        buf[sizeof buf - 1] = 0;
        int printable = 1;
        for (size_t i = 0; i < sizeof buf - 1 && buf[i]; ++i)
            if (buf[i] < 32 || buf[i] > 126) {
                printable = 0;
                break;
            }
        if (printable && path_from_buffer(buf, want, out, n))
            return 1;
    }
    return 0;
}

/* Last resort that is still exact: the app keeps its loaded copy in
 * /data/TempProjects/<project>/<name>.wav. Match by frame count, which the
 * TrimParameter reports; only accept a unique match. */
static int wav_by_frames(unsigned want, char *out, size_t n)
{
    const char *root = "/data/TempProjects";
    DIR *d = opendir(root);
    if (!d)
        return 0;
    struct dirent *de;
    char path[512];
    int found = 0;
    while ((de = readdir(d))) {
        if (de->d_name[0] == '.')
            continue;
        char sub[512];
        snprintf(sub, sizeof sub, "%s/%s", root, de->d_name);
        DIR *d2 = opendir(sub);
        if (!d2)
            continue;
        struct dirent *f;
        while ((f = readdir(d2))) {
            size_t len = strlen(f->d_name);
            if (len < 5 || strcasecmp(f->d_name + len - 4, ".wav"))
                continue;
            snprintf(path, sizeof path, "%s/%s", sub, f->d_name);
            unsigned fr = 0, rt = 0, ch = 0, doff = 0, bps = 2;
            int ok = wav_info(path, &fr, &rt, &ch, &doff, &bps);
            static int logged;
            if (logged < 10) {
                logged++;
                mlog("mpcmini: wav '%s' ok=%d frames=%u ch=%u\n", f->d_name, ok,
                     fr, ch);
            }
            if (!ok || fr != want)
                continue;
            if (found) {
                closedir(d2);
                closedir(d);
                mlog("mpcmini: autotrim: %u frames matches more than one wav\n", want);
                return 0;
            }
            snprintf(out, n, "%s", path);
            found = 1;
        }
        closedir(d2);
    }
    closedir(d);
    if (found)
        mlog("mpcmini: autotrim: wav by frames -> %s\n", out);
    return found;
}

/* The page binds each trim parameter through a small binding object whose
 * vtable is app+0x6e08a10 and which keeps the property pointer at +0x20. */
#define A_BINDING_VT 0x6e08a10

static uintptr_t find_prop_by_binding(float cur)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t want = base + A_BINDING_VT, hit = 0;
    for (int r = 0; r < g_nrng && !hit; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && !hit; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && !hit; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q != want)
                    continue;
                uintptr_t obj = a + o, p20 = 0;
                if (!rd(obj + 0x20, &p20, 8) || !p20)
                    continue;
                uintptr_t cands[2] = {p20 & ~(uintptr_t)0xf,
                                      (p20 & ~(uintptr_t)0xf) - 0x10};
                for (int k = 0; k < 2 && !hit; ++k) {
                    uintptr_t c = cands[k];
                    float v50 = 0;
                    if ((c & 7) || !c || !mapped((void *)c))
                        continue;
                    if (!rd(c + 0x50, &v50, 4) ||
                        !(v50 >= -0.001f && v50 <= 1.001f))
                        continue;
                    float d = v50 - cur;
                    if (d < 0)
                        d = -d;
                    if (d <= 0.02f)
                        hit = c;
                }
            }
        }
    }
    free(buf);
    return hit;
}

/* Properties seen in a real broadcast; the newest 256 are kept. */
static struct {
    uintptr_t p;
    float v;
} g_bc[256];
static volatile int g_nbc;

/* param -> property learned from a knob move (x20 inside the broadcaster). */
static struct {
    uintptr_t param;
    uintptr_t prop;
} g_pm[8];
static volatile int g_npm;

static uintptr_t learned_prop(uintptr_t param)
{
    for (int i = 0; i < g_npm; ++i)
        if (g_pm[i].param == param)
            return g_pm[i].prop;
    return 0;
}

/* A property seen in a real broadcast whose cached value is exactly the
 * parameter's current value is the one that owns it. Exact equality keeps the
 * false positives (and the abort they caused) away. */
static uintptr_t broadcast_prop(uintptr_t param, float cur)
{
    (void)param;
    uintptr_t best = 0;
    int hits = 0;
    for (int i = 0; i < (g_nbc < 256 ? g_nbc : 256); ++i) {
        uintptr_t p = g_bc[i].p;
        if ((p & 7) || p < 0x10000 || !mapped((void *)p))
            continue;
        if (g_bc[i].v != cur)
            continue;
        float v50 = 0;
        if (!rd(p + 0x50, &v50, 4) || v50 != cur)
            continue;
        if (best && p != best)
            hits++;
        best = p;
    }
    return hits ? 0 : best; /* ambiguous -> refuse */
}

/* ------------------------------------------------------------ app screens */
static int is_info_struct(uintptr_t info)
{
    unsigned short ch = 0;
    unsigned fr = 0, rt = 0;
    if (!info || !rd(info, &ch, 2))
        return 0;
    rd(info + 4, &fr, 4);
    rd(info + 8, &rt, 4);
    return (ch == 1 || ch == 2) && fr > 0 && rt >= 8000 && rt <= 192000;
}

static int holder_wav_path(uintptr_t obj, char *out, size_t n)
{
    char buf[512];
    if (!rd(obj, buf, sizeof buf - 1))
        return 0;
    buf[sizeof buf - 1] = 0;
    return path_from_buffer(buf, 0, out, n) || 0;
}

static int find_displayed_sample(unsigned *frames, char *path, size_t pathsz);
static void *find_sample_screen(void)
{
    ranges_load();
    uintptr_t want = base + 0x6e212a0; /* SampleModeScreen vtable */
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    void *found = 0;
    for (int r = 0; r < g_nrng && !found; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && !found; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && g_ntp < 256; o += 8) {
                uintptr_t v;
                memcpy(&v, buf + o, 8);
                if (v == want) {
                    found = (void *)(a + o);
                    break;
                }
            }
        }
    }
    free(buf);
    return found;
}

static int find_displayed_sample(unsigned *frames, char *path, size_t pathsz)
{
    void *screen = find_sample_screen();
    if (!screen) {
        mlog("mpcmini: no Sample screen found\n");
        return 0;
    }
    for (int off = 0; off < 0x800; off += 8) {
        uintptr_t child = 0;
        if (!rd((uintptr_t)screen + off, &child, 8) || !child)
            continue;
        if (!mapped((void *)child) || child < 0x10000)
            continue;
        uintptr_t info = 0;
        if (!rd(child + 0x130, &info, 8) || !is_info_struct(info))
            continue;
        char p2[512] = {0};
        for (int f = 0; f < 0x1000; f += 8) {
            uintptr_t cand = 0;
            if (!rd(child + f, &cand, 8) || !cand || !mapped((void *)cand))
                continue;
            if (holder_wav_path(cand, p2, sizeof p2))
                break;
        }
        unsigned fr = 0, rate = 0;
        rd(info + 4, &fr, 4);
        rd(info + 8, &rate, 4);
        *frames = fr;
        snprintf(path, pathsz, "%s", p2);
        mlog("mpcmini: sample obj=%p info=%p frames=%u path='%s'\n", (void *)child,
             (void *)info, fr, p2[0] ? p2 : "?");
        return p2[0] ? 1 : 0;
    }
    mlog("mpcmini: displayed sample struct not found (screen=%p)\n", screen);
    return 0;
}

/* ------------------------------------------------------------- app hooks */
static void *(*orig_sixteen_ctor)(void *, void *, void *, void *);
static void (*orig_sixteen_proc)(void *, int, void *);
static void (*orig_pitch_off)(void *, uint64_t, uint64_t, int);
static void (*orig_plus)(void *);
static void (*orig_minus)(void *);
static void (*orig_buttons)(void *);
static void (*orig_note_on)(void *, int, int, void *, float);
static void (*orig_note_off)(void *, int, int);
static void (*orig_cs)(void *, int, void *);
static void (*orig_trim_write)(void *, long, long);
static void (*orig_frames)(void *);
static void (*orig_trim)(void *, float);
static void (*orig_resized)(void *, long, long, long);
static void (*orig_reg)(void *, void *);
static void *(*orig_prog_inst)(void *, int);
static void (*orig_rotary)(void *, float);
static void (*orig_bcast)(void *, float);
static void (*orig_cc)(void *, unsigned, float, float);
static uintptr_t g_bp[8];
static volatile int g_nbp;

/* AC50WaveformLocator::bounds (0x16b6060): x = pos[+0x168] * view->w[+0x28] - 2.
 * Hooking it records the locators that are actually painting, which is the only
 * way to know which objects the page positions. */
#define MAX_LOC 12
static uintptr_t g_loc[MAX_LOC];
static volatile int g_nloc;
static void (*orig_loc_bounds)(void *);

static void loc_bounds(void *self)
{
    uintptr_t s = (uintptr_t)self;
    if (s) {
        int i, known = 0;
        for (i = 0; i < g_nloc; ++i)
            if (g_loc[i] == s) {
                known = 1;
                break;
            }
        if (!known && g_nloc < MAX_LOC) {
            float pos = 0;
            rd(s + 0x168, &pos, 4);
            g_loc[g_nloc++] = s;
            mlog("mpcmini: locator %p pos=%.4f\n", (void *)s, (double)pos);
        }
    }
    if (orig_loc_bounds)
        orig_loc_bounds(self);
}
static void (*orig_obs_set)(void *, float);
static void (*orig_str_assign)(void *, const char *);
static void *(*orig_label_text)(void *);
static void (*orig_draw_text)(void *, const uint64_t *, int, int, int, int, int, int, float);
static void (*orig_repaint)(void *, uint64_t, uint64_t);

static void set_filter_text(const char *t);
static void do_autotrim(void);
static void play_note(int pitch, int on, float velocity);

static volatile int g_shift;
static volatile int g_trim_req;
static volatile int g_last_pad = -1;
static volatile int g_core_hit;
static void *g_props;
static int g_dbg;

static void *sixteen_ctor(void *a, void *b, void *c, void *d)
{
    void *self = orig_sixteen_ctor ? orig_sixteen_ctor(a, b, c, d) : 0;
    g_sixteen = self ? self : a;
    if (debug_mode && g_dbg < 40) {
        g_dbg++;
        mlog("mpcmini: 16 levels ctor %p\n", g_sixteen);
    }
    return self;
}

/* The stock code computes the 16 Levels pitch here; shift the whole row. */
static void sixteen_proc(void *self, int idx, void *out)
{
    /* pure pass-through: the pitch is set by A_PITCH_OFF, not by writing
     * through the third argument (it is a context object, not a float). */
    if (orig_sixteen_proc)
        orig_sixteen_proc(self, idx, out);
}

static void pitch_off(void *a0, uint64_t a1, uint64_t a2, int a3)
{
    int off = a3 + 8 * g_octave; /* half of the 16-note grid per press */
    if (g_octave && debug_mode)
        mlog("mpcmini: pitch offset %d -> %d\n", a3, off);
    if (orig_pitch_off)
        orig_pitch_off(a0, a1, a2, off);
}

static void heap_snapshot(const char *tag);
static void heap_diff(const char *tag);
static uintptr_t g_last_press_obj;
static void find_float_pair(float v);
static volatile int g_poke_which = -1;
static void do_poke(void);
static volatile float g_poke_value;
static float g_mark_v0 = -1.0f, g_mark_v1 = -1.0f;
static volatile int g_pending_cc;
static volatile int g_in_inject;
static void commit_trim(void);

static void buttons(void *event)
{

    unsigned code = event ? FIELD(event, 8, uint8_t) : 0;
    unsigned down = event ? FIELD(event, 9, uint8_t) : 0;
    if (code == 0x31) { /* SHIFT */
        g_shift = down ? 1 : 0;
        set_filter_text(down ? "AutoTrim" : "Filter");
        if (orig_buttons)
            orig_buttons(event);
        return;
    }
    if (down && g_shift && code == 0x0e) { /* SHIFT + B3 */
        mlog("mpcmini: combo SHIFT+B3 -> autotrim\n");
        /* Analysis is cheap now (block reads); it must stay on this thread so
         * the commit is queued for the very next repaint. */
        do_autotrim();
        return;
    }
    /* PLUS / MINUS move the 16 Levels row by an octave. The pitch hook only
     * runs for that screen, so bumping unconditionally is safe - and the old
     * guard (g_octave != 0) made the first press a no-op. */
    if (getenv("MPCMINI_BTNLOG")) {
        static int n;
        if (n < 24) {
            n++;
            unsigned char raw[16];
            if (rd((uintptr_t)event, raw, sizeof raw)) {
                mlog("mpcmini: btn ev="
                     "%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x\n",
                     raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
                     raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14],
                     raw[15]);
            }
        }
    }
    do_poke();
    if (down && g_mark_v0 >= 0.0f) {
        /* any key press may change what the page shows: drop our edges and ask
         * for a repaint so they actually disappear */
        g_mark_v0 = -1.0f;
        g_mark_v1 = -1.0f;
        if (g_screen)
            FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
                g_screen, 0, FIELD(g_screen, 0x28, uint64_t));
    }
    if (code == 0x06 && down == 0x7f) {
        /* remember the object of the last press so it can be replayed to make
         * the sample page reload (which is when it redraws its trim) */
        uintptr_t ob = 0;
        if (rd((uintptr_t)event, &ob, 8) && ob)
            g_last_press_obj = ob;
    }
    if (code == 0x06) {
        static int k06;
        if (k06 < 10) {
            k06++;
            unsigned char raw[16];
            if (rd((uintptr_t)event, raw, sizeof raw))
                mlog("mpcmini: ev06 %02x %02x %02x %02x %02x %02x %02x %02x | "
                     "%02x %02x %02x %02x %02x %02x %02x %02x\n",
                     raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
                     raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14],
                     raw[15]);
        }
    }
    if (code == 0x36 || code == 0x37) {
        int plus = (code == 0x36);
        if (down) {
            /* both keys at once = reset the row to its original pitch */
            if (plus ? g_minus_held : g_plus_held) {
                g_octave = 0;
                mlog("mpcmini: PLUS+MINUS -> pitch reset to original\n");
            } else {
                bump_octave(plus ? +1 : -1);
            }
            if (plus)
                g_plus_held = 1;
            else
                g_minus_held = 1;
        } else {
            if (plus)
                g_plus_held = 0;
            else
                g_minus_held = 0;
        }
        return; /* swallow: the stock action would nudge the selected knob */
    }
    if (debug_mode && g_dbg < 80) {
        g_dbg++;
        mlog("mpcmini: button code=0x%02x down=%u\n", code, down);
    }
    if (orig_buttons)
        orig_buttons(event);
}

static int g_replaying_pad;
static struct {
    void *input;
    int src, pad;
    void *props;
    float vel;
    int valid;
} g_pad_rec[128];

static void note_on(void *input, int src, int pad, void *props, float vel)
{
    g_last_pad = pad;
    g_props = props;

    if (!g_replaying_pad && pad >= 0 && pad < 128) {
        /* remember a real pad press so the page can be re-selected later */
        g_pad_rec[pad].input = input;
        g_pad_rec[pad].src = src;
        g_pad_rec[pad].pad = pad;
        g_pad_rec[pad].props = props;
        g_pad_rec[pad].vel = vel;
        g_pad_rec[pad].valid = 1;
    }
    if (debug_mode && g_dbg < 120) {
        g_dbg++;
        mlog("mpcmini: note_on pad=%d vel=%.2f\n", pad, (double)vel);
    }
    if (orig_note_on)
        orig_note_on(input, src, pad, props, vel);
}

static void note_off(void *input, int src, int pad)
{
    if (orig_note_off)
        orig_note_off(input, src, pad);
}

static void *g_cs_self;

static void cs_event(void *self, int code, void *payload)
{
    if (self)
        g_cs_self = self;
    if (debug_mode && g_dbg < 160) {
        g_dbg++;
        mlog("mpcmini: cs code=0x%02x\n", code & 0xff);
    }
    if (orig_cs)
        orig_cs(self, code, payload);
}

static void trim_write_probe(void *self, long a, long b)
{
    if (orig_trim_write)
        orig_trim_write(self, a, b);
}

static void frames_probe(void *self)
{
    if (orig_frames)
        orig_frames(self);
}

static void screen_resized(void *a, long b, long c, long d)
{
    if (orig_resized)
        orig_resized(a, b, c, d);
}

static void trim_reg(void *a, void *b)
{
    if (orig_reg)
        orig_reg(a, b);
}

static void *prog_inst(void *a, int b)
{
    return orig_prog_inst ? orig_prog_inst(a, b) : 0;
}

static void rotary(void *a, float v)
{
    if (orig_rotary)
        orig_rotary(a, v);
}

/* The trim core is called by the broadcaster's listener loop, which keeps the
 * property in x20: learning that pair is what lets autotrim drive the page. */
static volatile int g_own_call;
static struct { uintptr_t param, listener; } g_lis[8];
static volatile int g_nlis;
static struct { uintptr_t param, slider; } g_ps[8];
static volatile int g_nps;

static uintptr_t slider_for_value(float pct)
{
    if (!g_screen)
        return 0;
    const size_t len = 0x20000;
    unsigned char *buf = malloc(len);
    if (!buf)
        return 0;
    if (!rd((uintptr_t)g_screen, buf, len)) {
        free(buf);
        return 0;
    }
    uintptr_t want = base + 0x706e5d0, found = 0;
    for (size_t o = 0; o + 8 <= len; o += 8) {
        uintptr_t q;
        memcpy(&q, buf + o, 8);
        if (q != want)
            continue;
        size_t vo = o + 0x1b4;
        if (vo + 4 > len)
            continue;
        float v;
        memcpy(&v, buf + vo, 4);
        float d = v - pct;
        if (d < 0)
            d = -d;
        if (d <= 1.0f) {
            found = (uintptr_t)g_screen + o;
            break;
        }
    }
    free(buf);
    return found;
}

static void *g_obs[8];
static int g_nobs;

static void obs_set(void *self, float v)
{
    /* records the property objects the control surface writes */
    if (!g_own_call) {
        int seen = 0;
        for (int i = 0; i < g_nobs; ++i)
            if (g_obs[i] == self)
                seen = 1;
        if (!seen && g_nobs < 8) {
            g_obs[g_nobs++] = self;
            mlog("mpcmini: obs property=%p v=%.4f\n", self, (double)v);
        }
    }
    if (orig_obs_set)
        orig_obs_set(self, v);
}

static void trim_probe(void *self, float v)
{
    static int logs;
    if (g_own_call) {
        if (orig_trim)
            orig_trim(self, v);
        return;
    }
    uintptr_t prop = 0;
    __asm__ volatile("mov %0, x20" : "=r"(prop));
    if (probe_mode) {
        uintptr_t regs[8];
        __asm__ volatile("mov %0, x19" : "=r"(regs[0]));
        __asm__ volatile("mov %0, x20" : "=r"(regs[1]));
        __asm__ volatile("mov %0, x21" : "=r"(regs[2]));
        __asm__ volatile("mov %0, x22" : "=r"(regs[3]));
        __asm__ volatile("mov %0, x23" : "=r"(regs[4]));
        __asm__ volatile("mov %0, x24" : "=r"(regs[5]));
        __asm__ volatile("mov %0, x25" : "=r"(regs[6]));
        __asm__ volatile("mov %0, x26" : "=r"(regs[7]));
        static int rl;
        if (rl < 4) {
            rl++;
            for (int i = 0; i < 8; ++i) {
                uintptr_t r = regs[i];
                float c50 = 0;
                int ok = r > 0x10000 && mapped((void *)r) && rd(r + 0x50, &c50, 4) &&
                         c50 >= -0.001f && c50 <= 1.001f;
                mlog("mpcmini: reg x%2d=%p propish=%d c50=%.4f\n", 19 + i, (void *)r,
                     ok, ok ? (double)c50 : 0.0);
            }
        }
    }
    g_core_hit = 1;
    {
        uintptr_t sl = slider_for_value(v * 100.0f);
        if (sl) {
            int i;
            for (i = 0; i < g_nps; ++i)
                if (g_ps[i].param == (uintptr_t)self) {
                    g_ps[i].slider = sl;
                    break;
                }
            if (i == g_nps && g_nps < 8) {
                g_ps[g_nps].param = (uintptr_t)self;
                g_ps[g_nps].slider = sl;
                g_nps++;
            }
            mlog("mpcmini: knob param=%p slider=%p (%.2f%%)\n", self, (void *)sl,
                 (double)(v * 100.0f));
        }
    }
    if (prop && mapped((void *)prop)) {
        int slot = -1;
        for (int i = 0; i < g_nlis; ++i)
            if (g_lis[i].param == (uintptr_t)self) { slot = i; break; }
        if (slot < 0 && g_nlis < 8) { slot = g_nlis++; g_lis[slot].param = (uintptr_t)self; }
        if (slot >= 0) g_lis[slot].listener = prop;
    }
    if (probe_mode && prop && mapped((void *)prop)) {
        float v50 = 0;
        int has = rd(prop + 0x50, &v50, 4);
        int refs = prop_references(prop, (uintptr_t)self);
        static int diag;
        if (diag < 6) {
            diag++;
            mlog("mpcmini: diag prop=%p v50=%s%.4f refs=%d v=%.4f\n", (void *)prop,
                 has ? "" : "?", (double)v50, refs, (double)v);
        }
    }
    if (prop && mapped((void *)prop)) {
        float v50 = 0;
        /* When x20 really is the property (its cache equals the value), dump
         * the listener list: that is the set of objects the page updates. */
        if (rd(prop + 0x50, &v50, 4)) {
            float d = v50 - v;
            if (d < 0)
                d = -d;
            if (d <= 0.001f) {
                static int walks;
                if (walks < 2) {
                    walks++;
                    uintptr_t head = 0, node = 0;
                    rd(prop + 0x10, &head, 8);
                    node = head;
                    mlog("mpcmini: prop %p head=%p sentinel=%p\n", (void *)prop,
                         (void *)head, (void *)(prop + 8));
                    for (int i = 0; i < 8 && node && node != prop + 8; ++i) {
                        uintptr_t tgt = 0, next = 0;
                        if (!rd(node + 0x18, &tgt, 8) || !rd(node + 8, &next, 8))
                            break;
                        mlog("mpcmini:   node %p target=%p next=%p\n", (void *)node,
                             (void *)(tgt & ~(uintptr_t)3), (void *)next);
                        node = next;
                    }
                }
            }
        }
        static int learn_diag;
        if (learn_diag < 12) {
            learn_diag++;
            int has = rd(prop + 0x50, &v50, 4);
            mlog("mpcmini: learn? prop=%p c50=%s%.4f v=%.4f\n", (void *)prop,
                 has ? "" : "?", (double)v50, (double)v);
        }
        if (rd(prop + 0x50, &v50, 4) && v50 >= -0.001f && v50 <= 1.001f) {
            float d = v50 - v;
            if (d < 0)
                d = -d;
            if (d <= 0.02f) {
                int i;
                for (i = 0; i < g_npm; ++i)
                    if (g_pm[i].param == (uintptr_t)self)
                        break;
                if (i == g_npm && g_npm < 8)
                    g_npm++;
                if (i < 8) {
                    g_pm[i].param = (uintptr_t)self;
                    g_pm[i].prop = prop;
                }
                if (probe_mode && logs < 6) {
                    logs++;
                    mlog("mpcmini: learnt self=%p prop=%p v=%.4f\n", self,
                         (void *)prop, (double)v);
                }
            }
        }
    }
    if (!g_own_call) {
        /* a real knob move takes over: drop our markers */
        g_mark_v0 = -1.0f;
        g_mark_v1 = -1.0f;
    }
    if (probe_mode && logs < 200) {
        uintptr_t lay = 0, ins = 0;
        if (page_layers((uintptr_t)self, &lay, &ins)) {
            unsigned a = 0, b = 0, c = 0;
            rd(lay + 0x4d8, &a, 4);
            rd(lay + 0x530, &b, 4);
            rd(lay + 0x588, &c, 4);
            mlog("mpcmini: layer@knob %p 4d8=%u 530=%u 588=%u\n", (void *)lay, a, b, c);
        }
        uintptr_t vt = 0, val = 0, ap = 0;
        rd((uintptr_t)self, &vt, 8);
        rd((uintptr_t)self + T_VALUE, &val, 4);
        rd((uintptr_t)self + T_APPLIED, &ap, 4);
        logs++;
        mlog("mpcmini: trim self=%p vptr=%p(base+0x%lx) v=%.4f val=%.4f ap=%.4f lr=%p\n",
             self, (void *)vt, vt >= base ? (unsigned long)(vt - base) : 0UL,
             (double)v, (double)val, (double)ap, __builtin_return_address(0));
    }
    if (orig_trim)
        orig_trim(self, v);
}

static void bcast_hook(void *self, float v)
{
    int i = g_nbc;
    g_bc[i & 255].p = (uintptr_t)self;
    g_bc[i & 255].v = v;
    g_nbc = i + 1;
    if (probe_mode && i < 64)
        mlog("mpcmini: bcast %p v=%.4f\n", self, (double)v);
    /* Record the property a *native* broadcast uses: that is the object the
     * page listens to. Our own broadcasts are flagged. */
    if (!g_own_call) {
        static int lrc;
        int nice = (v > 0.001f && v < 0.999f) && v != 0.5f;
        if (nice && lrc < 24) {
            lrc++;
            mlog("mpcmini: bcast lr=%p prop=%p v=%.4f\n",
                 __builtin_return_address(0), self, (double)v);
        }
        uintptr_t P = (uintptr_t)self;
        int known = 0;
        for (int k = 0; k < g_nbp; ++k)
            if (g_bp[k] == P) {
                known = 1;
                break;
            }
        if (!known && g_nbp < 8) {
            g_bp[g_nbp++] = P;
            mlog("mpcmini: native bcast property=%p v=%.4f\n", self, (double)v);
        }
    }
    if (orig_bcast)
        orig_bcast(self, v);
}

/* ------------------------------------------------------- message-thread pump */
static volatile int g_trim_pending;
static void *g_trim_param0, *g_trim_param1;
static float g_trim_v0, g_trim_v1;
static float g_trim_cur0, g_trim_cur1;
static unsigned g_trim_frames;
static uintptr_t g_trim_layer, g_trim_screen;

/* Broadcast a unique value to one candidate property: if the app's own
 * listener chain reaches the trim core for *our* parameter, that property owns
 * it. The original value is restored right after. */
static uintptr_t g_cand[24];
static int g_ncand;

/* The page keeps a small binding object per parameter (vtable app+0x6e08a10)
 * that holds the property at +0x20. Collect those as probe candidates. */
static void collect_binding_props(void)
{
    if (g_ncand)
        return;
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return;
    uintptr_t want = base + A_BINDING_VT;
    for (int r = 0; r < g_nrng && g_ncand < 24; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && g_ncand < 24;
             a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && g_ncand < 24; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q != want)
                    continue;
                uintptr_t p20 = 0;
                if (!rd(a + o + 0x20, &p20, 8) || !p20)
                    continue;
                uintptr_t cs[2] = {p20 & ~(uintptr_t)0xf, (p20 & ~(uintptr_t)0xf) - 0x10};
                for (int k = 0; k < 2; ++k) {
                    uintptr_t c = cs[k];
                    float f = 0;
                    if (!c || (c & 7) || !mapped((void *)c))
                        continue;
                    if (!rd(c + 0x50, &f, 4) || !(f >= -0.001f && f <= 1.001f))
                        continue;
                    /* statics in the app image pass the float test but are not
                     * properties; a property lives on the heap */
                    if (c >= base && c < base + 0x9000000)
                        continue;
                    int dup = 0;
                    for (int i = 0; i < g_ncand; ++i)
                        if (g_cand[i] == c) { dup = 1; break; }
                    if (!dup)
                        g_cand[g_ncand++] = c;
                }
            }
        }
    }
    free(buf);
    mlog("mpcmini: %d property candidates from bindings\n", g_ncand);
}

/* Read-only ownership test: the property's listener chain must reference the
 * parameter (or its model) somewhere within two hops. No broadcasting, so a
 * wrong candidate can never disturb the app. */
static int prop_references(uintptr_t P, uintptr_t param)
{
    uintptr_t model = 0;
    rd(param + T_MODEL, &model, 8);
    uintptr_t node = 0;
    if (!rd(P + 0x10, &node, 8) || !node)
        return 0;
    uintptr_t back = 0;
    if (!rd(node, &back, 8) || back != P + 0x10)
        return 0;
    unsigned char buf[0x200];
    unsigned char buf2[0x100];
    for (int n = 0; n < 16; ++n) {
        if (node == P + 8 || !node || !mapped((void *)node))
            break;
        uintptr_t tgt = 0;
        if (!rd(node + 0x18, &tgt, 8))
            break;
        tgt &= ~(uintptr_t)3;
        if (tgt && mapped((void *)tgt) && rd(tgt, buf, sizeof buf)) {
            for (size_t o = 0; o + 8 <= sizeof buf; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (!q)
                    continue;
                if (q == param || (model && q == model))
                    return 1;
                if (q < 0x10000 || !mapped((void *)q))
                    continue;
                if (!rd(q, buf2, sizeof buf2))
                    continue;
                for (size_t k = 0; k + 8 <= sizeof buf2; k += 8) {
                    uintptr_t q2;
                    memcpy(&q2, buf2 + k, 8);
                    if (q2 == param || (model && q2 == model))
                        return 1;
                }
            }
        }
        uintptr_t next = 0;
        if (!rd(node + 8, &next, 8))
            break;
        node = next;
    }
    return 0;
}

/* The object in x20 at a trim-core call is the *listener*, not the property.
 * The property that owns it is the one whose list contains a node whose target
 * is exactly that object. */
static uintptr_t owner_of_listener(uintptr_t lis)
{
    if (!lis)
        return 0;
    for (int i = 0; i < g_ncand; ++i) {
        uintptr_t P = g_cand[i];
        uintptr_t node = 0;
        if (!rd(P + 0x10, &node, 8) || !node || !mapped((void *)node))
            continue;
        for (int n = 0; n < 16; ++n) {
            uintptr_t tgt = 0;
            if (!rd(node + 0x18, &tgt, 8))
                break;
            tgt &= ~(uintptr_t)3;
            if (tgt == lis)
                return P;
            uintptr_t next = 0;
            if (!rd(node + 8, &next, 8) || next == P + 8 || !next)
                break;
            node = next;
        }
    }
    return 0;
}

static uintptr_t listener_of_param(uintptr_t param)
{
    for (int i = 0; i < g_nlis; ++i)
        if (g_lis[i].param == param)
            return g_lis[i].listener;
    return 0;
}

static int probe_property(uintptr_t param)
{
    collect_binding_props();
    for (int i = 0; i < g_ncand; ++i)
        if (prop_references(g_cand[i], param)) {
            int slot = g_npm;
            if (slot < 8) {
                g_pm[slot].param = param;
                g_pm[slot].prop = g_cand[i];
                g_npm = slot + 1;
            }
            mlog("mpcmini: owner of param=%p is prop=%p\\n", (void *)param,
                 (void *)g_cand[i]);
            return 1;
        }
    return 0;
}

/* am-mod's lesson: don't write the model, drive the app's own input path.
 * The Trim knobs are control-surface events 0x10 (Start) / 0x11 (End) with an
 * absolute 7-bit position, so the app updates parameter, page and audio. */
static int inject_cc(unsigned code, unsigned value)
{
    if (!orig_buttons)
        return 0;
    unsigned char ev[16] = {0};
    ev[8] = (unsigned char)code;
    /* sweep like the physical encoder: the app treats these as movements */
    for (unsigned v = 0; v <= 127; v += 1) {
        ev[9] = (unsigned char)v;
        orig_buttons(ev);
    }
    ev[9] = (unsigned char)(value > 127 ? 127 : value);
    orig_buttons(ev);
    return 1;
}

/* am-mod sets native properties through the command at 0x1671650 with a pointer
 * to the value; the property lives inside SamplePadControls (root+0x178) and
 * has guards at +0x38/+0x78/+0x80/+0x88. Find the one that references our trim
 * parameter, then set it the same way. */
static uintptr_t find_controls_prop(uintptr_t param)
{
    if (!g_screen)
        return 0;
    /* am-mod's SamplePadControls offset belongs to its firmware; ours differs.
     * Scan the screen object itself for the same property shape. */
    uintptr_t controls = (uintptr_t)g_screen;
    for (long o = 0; o < 0x12000; o += 8) {
        uintptr_t p = controls + (uintptr_t)o;
        uintptr_t f38 = 0, f78 = 0, f80 = 0, f88 = 0;
        if (!rd(p + 0x38, &f38, 8) || !rd(p + 0x78, &f78, 8) ||
            !rd(p + 0x80, &f80, 8) || !rd(p + 0x88, &f88, 8))
            break;
        if (!f38 || !f78 || !f80 || !f88)
            continue;
        if (!mapped((void *)f38) || !mapped((void *)f78))
            continue;
        unsigned char buf[0x100];
        if (!rd(p, buf, sizeof buf))
            continue;
        for (size_t k = 0; k + 8 <= sizeof buf; k += 8) {
            uintptr_t q;
            memcpy(&q, buf + k, 8);
            if (q == param) {
                mlog("mpcmini: controls prop +0x%lx references %p\n", (unsigned long)o,
                     (void *)param);
                return p;
            }
        }
    }
    return 0;
}

/* The page draws its locators from the trim **slider widget**: a ui::Slider
 * embedded in the screen holds its position as a percentage at +0x1b4
 * (measured: +0x6758 = 49.0 right after K1 was set to 50). Writing that value
 * and repainting is what a knob move does to the widget. */
static int set_slider_percent(float from, float to)
{
    if (!g_screen)
        return 0;
    uintptr_t want = base + 0x706e5d0;
    for (long o = 0; o < 0x20000; o += 8) {
        uintptr_t q = 0;
        if (!rd((uintptr_t)g_screen + o, &q, 8))
            break;
        if (q != want)
            continue;
        uintptr_t s = (uintptr_t)g_screen + (uintptr_t)o;
        float v = 0;
        if (!rd(s + 0x1b4, &v, 4))
            continue;
        float d = v - from;
        if (d < 0)
            d = -d;
        if (d <= 0.75f) {
            wr(s + 0x1b4, &to, 4);
            mlog("mpcmini: slider +0x%lx %.2f -> %.2f\n", o, (double)v, (double)to);
            return 1;
        }
    }
    return 0;
}

/* Apply the trim to one screen instance: find the two trim sliders by the
 * instance's own cached percentages, write ours into their +0x1b4 and repaint.
 * Returns 1 when both were found. */
static int apply_to_instance(uintptr_t S, float p0, float p1)
{
    float c0 = 0, c1 = 0;
    if (!rd(S + 0x1f4c, &c0, 4) || !rd(S + 0x1f50, &c1, 4))
        return 0;
    if (!(c0 >= -0.5f && c0 <= 100.5f && c1 >= -0.5f && c1 <= 100.5f))
        return 0;
    uintptr_t want = base + 0x706e5d0, s0 = 0, s1 = 0;
    for (long o = 0; o < 0x20000; o += 8) {
        uintptr_t q = 0;
        if (!rd(S + o, &q, 8))
            break;
        if (q != want)
            continue;
        uintptr_t sl = S + (uintptr_t)o;
        float v = 0;
        if (!rd(sl + 0x1b4, &v, 4))
            continue;
        float d0 = v - c0, d1 = v - c1;
        if (d0 < 0) d0 = -d0;
        if (d1 < 0) d1 = -d1;
        if (!s0 && d0 <= 0.6f)
            s0 = sl;
        else if (!s1 && d1 <= 0.6f)
            s1 = sl;
    }
    if (!s0 || !s1 || s0 == s1)
        return 0;
    wr(s0 + 0x1b4, (void *)&p0, 4);
    wr(s1 + 0x1b4, (void *)&p1, 4);
    mlog("mpcmini: instance %p sliders %p/%p -> %.2f/%.2f (was %.2f/%.2f)\n",
         (void *)S, (void *)s0, (void *)s1, (double)p0, (double)p1, (double)c0,
         (double)c1);
    FN(0x1149880, void (*)(void *, uint64_t, uint64_t))((void *)S, 0,
                                                        FIELD(S, 0x28, uint64_t));
    return 1;
}

/* The page paints from the *model*. Find the TrimParameter instances whose
 * value matches the page's own cached percentages (screen+0x1f4c / +0x1f50):
 * those are the parameters the page is actually showing. */
static uintptr_t param_by_value(float frac)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t want = base + A_TRIM_PARAM_VT, best = 0;
    float bestd = 0.02f;
    for (int r = 0; r < g_nrng; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && g_ntp < 256; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && g_ntp < 256; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q != want)
                    continue;
                uintptr_t p = a + o;
                float v = 0;
                if (!rd(p + T_VALUE, &v, 4))
                    continue;
                float d = v - frac;
                if (d < 0)
                    d = -d;
                if (d < bestd) {
                    bestd = d;
                    best = p;
                }
            }
        }
    }
    free(buf);
    return best;
}

/* Find the page object by its own cached percentage: a float equal to the
 * End percent whose address-0x1f50 starts with a vtable pointer. Works for any
 * screen subclass, which is what the enumeration by one vptr misses. */
static uintptr_t find_page_by_cache(float pct)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t hit = 0;
    for (int r = 0; r < g_nrng && !hit; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && !hit; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 4 <= n && !hit; o += 4) {
                float v;
                memcpy(&v, buf + o, 4);
                if (v < 0.5f || v > 100.5f)
                    continue;
                float d = v - pct;
                if (d < 0) d = -d;
                if (d > 0.75f)
                    continue;
                uintptr_t addr = a + o;
                uintptr_t b = addr - 0x1f50;
                /* the page is a heap object, not a static in the image */
                if (b >= base && b < base + 0x9000000)
                    continue;
                uintptr_t vt = 0;
                if (b < 0x10000 || !rd(b, &vt, 8))
                    continue;
                if (vt < base + 0x6ddf910 || vt > base + 0x7162018)
                    continue;
                float chk = 0;
                if (!rd(b + 0x1f4c, &chk, 4))
                    continue;
                if (chk < -0.5f || chk > 100.5f)
                    continue;
                hit = b;
                mlog("mpcmini: page base %p vptr=%p (vt+0x%lx)\n", (void *)b,
                     (void *)vt, (unsigned long)(vt - base));
            }
        }
    }
    free(buf);
    return hit;
}

/* Enumerate objects of the Trim page class and pick the one whose cached
 * percentages match what the layer currently holds. */
static uintptr_t find_trim_page(float c0, float c1)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t want = base + A_TRIM_PAGE_VT, best = 0;
    float bestscore = 3.0f;
    for (int r = 0; r < g_nrng; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && g_ntp < 256; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && g_ntp < 256; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q != want)
                    continue;
                uintptr_t p = a + o;
                float x = 0, y = 0;
                if (!rd(p + 0x1f4c, &x, 4) || !rd(p + 0x1f50, &y, 4))
                    continue;
                if (!(x >= -0.5f && x <= 100.5f && y >= -0.5f && y <= 100.5f))
                    continue;
                float d0 = x - c0, d1 = y - c1;
                if (d0 < 0) d0 = -d0;
                if (d1 < 0) d1 = -d1;
                float score = d0 + d1;
                if (score < bestscore) {
                    bestscore = score;
                    best = p;
                }
            }
        }
    }
    free(buf);
    if (best)
        mlog("mpcmini: trim page %p (score %.2f)\n", (void *)best, (double)bestscore);
    return best;
}

/* Find the listener node that points at `param` and report what looks like its
 * owner property. A property P keeps its list head at P+0x10 and its sentinel
 * at P+8; a node keeps its target (tagged) at +0x18 and its next at +8. */
static void probe_node(uintptr_t param)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return;
    int hits = 0;
    for (int r = 0; r < g_nrng && hits < 4; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && hits < 4; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && hits < 4; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if ((q & ~(uintptr_t)3) != param)
                    continue;
                uintptr_t node = a + o - 0x18;
                uintptr_t owner = 0, next = 0, back = 0;
                rd(node, &owner, 8);
                rd(node + 8, &next, 8);
                rd(node + 0x10, &back, 8);
                mlog("mpcmini: node %p owner=%p next=%p back=%p\n", (void *)node,
                     (void *)owner, (void *)next, (void *)back);
                if (owner && mapped((void *)owner)) {
                    uintptr_t head = 0;
                    rd(owner + 0x10, &head, 8);
                    float c50 = 0;
                    int ok50 = rd(owner + 0x50, &c50, 4) && c50 >= -1.0f && c50 <= 1.0f;
                    mlog("mpcmini:   owner+0x10=%p c50=%.4f (%d)\n", (void *)head,
                         (double)c50, ok50);
                }
                hits++;
                break;
            }
        }
    }
    free(buf);
}

/* Recover a parameter's owning property without watching a knob.
 *
 * A property P keeps its list head at P+0x10 and its sentinel at P+8. A node
 * keeps its target (tagged) at +0x18 and its next at +8; the last next in the
 * chain is the sentinel, so P = sentinel - 8. Validation: P+0x10 must be a
 * mapped node and P+0x50 a plausible float. */
static uintptr_t prop_of_param(uintptr_t param)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t result = 0;
    for (int r = 0; r < g_nrng && !result; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && !result; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && !result; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if ((q & ~(uintptr_t)3) != param)
                    continue;
                uintptr_t node = a + o - 0x18;
                for (int step = 0; step < 8 && !result; ++step) {
                    uintptr_t next = 0;
                    if (!rd(node + 8, &next, 8) || !next || !mapped((void *)next))
                        break;
                    uintptr_t cand = next - 8;
                    uintptr_t head = 0;
                    float c50 = 0;
                    /* The image holds the class prototype of every property; it
                     * matches structurally but is never the live one. */
                    if (cand >= base && cand < base + 0x9000000)
                        ; /* reject below */
                    else if (rd(cand + 0x10, &head, 8) && head && mapped((void *)head) &&
                             rd(cand + 0x50, &c50, 4) && c50 >= -2.0f && c50 <= 2.0f) {
                        /* prove the chain from head reaches our node */
                        uintptr_t w = head;
                        for (int i = 0; i < 32 && w; ++i) {
                            if (w == node) {
                                result = cand;
                                break;
                            }
                            uintptr_t nx = 0;
                            if (!rd(w + 8, &nx, 8) || !nx || !mapped((void *)nx))
                                break;
                            if (nx == cand + 8)
                                break;
                            w = nx;
                        }
                    }
                    node = next;
                    if (node == cand + 8)
                        break;
                }
            }
        }
    }
    free(buf);
    return result;
}

/* The app's own CC path: 0x159f230(range, event, raw) divides raw by range and
 * broadcasts through the property at *(event[0]+0x10) + 0x1ac0 (CC 0x10 = K1)
 * or +0x1b18 (CC 0x11 = K2). Calling it is exactly a knob turn. */
static uintptr_t ctl_obj_for(uintptr_t base)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t hit = 0;
    for (int r = 0; r < g_nrng && !hit; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && !hit; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q == base) {
                    hit = a + o - 0x10;
                    break;
                }
            }
        }
    }
    free(buf);
    return hit;
}

/* A real CC event captured from the app, replayed with our own number so the
 * app performs exactly what a knob does. */
static unsigned char g_cc_ev[32];
static int g_cc_ev_valid;
static unsigned g_cc_raw[2];
static float g_cc_range[2] = {127.0f, 127.0f};
static int g_cc_seen[2];
static int g_in_cc_replay;

static void cc_capture(void *ev, unsigned raw, float lo, float range)
{
    (void)lo;
    if (!g_in_cc_replay && ev) {
        unsigned char cc = ((unsigned char *)ev)[8];
        static int allc;
        if (allc < 16) {
            allc++;
            mlog("mpcmini: cc_in cc=0x%x raw=%u range=%.1f ev0=%p\n", cc, raw,
                 (double)range, *(void **)ev);
        }
        int idx = -1;
        if (cc == 0x10)
            idx = 0;
        else if (cc == 0x11)
            idx = 1;
        if (idx >= 0) {
            if (!g_cc_ev_valid) {
                memcpy(g_cc_ev, ev, sizeof g_cc_ev);
                g_cc_ev_valid = 1;
                mlog("mpcmini: captured CC event (cc=0x%x raw=%u range=%.1f)\n", cc,
                     raw, (double)range);
            }
            g_cc_raw[idx] = raw;
            if (range > 1.0f)
                g_cc_range[idx] = range;
            g_cc_seen[idx] = 1;
        }
    }
    if (orig_cc)
        orig_cc(ev, raw, lo, range);
}

static void cc_replay(unsigned cc, unsigned raw)
{
    unsigned char ev[32];
    memcpy(ev, g_cc_ev, sizeof ev);
    ev[8] = (unsigned char)cc;
    g_in_cc_replay = 1;
    if (orig_cc)
        orig_cc(ev, raw, 0.0f, 127.0f);
    g_in_cc_replay = 0;
}

static void cc_inject(uintptr_t obj, unsigned cc, unsigned raw)
{
    /* Static, not stack: the app may keep the pointer it was handed (the abort
     * after the replay looked exactly like a dangling event). */
    static unsigned char ev[64];
    memset(ev, 0, sizeof ev);
    *(uintptr_t *)ev = obj;
    ev[8] = (unsigned char)cc;
    /* Disassembly of 0x159f230:
     *   ldrb w3,[x0,8]    -> x0 is the event, byte +8 is the CC
     *   and  w2,w1,0x7f   -> w1 is the raw 7-bit value
     *   fdiv s0,s0,s1     -> s1 is the range (so: two float parameters)
     *   ldr  x0,[x0] / ldr x0,[x0,0x10] / add x0,x0,#0x1ac0 -> the property
     * so the call is (event, raw, min, range). */
    FN(0x159f230, void (*)(void *, unsigned, float, float))(ev, raw, 0.0f, 127.0f);
}

/* Whole-heap fingerprint: hash every 4 KiB page of writable memory. Two
 * snapshots around a knob move show *everything* the knob changes (the model,
 * not just the parameter), which is what the page reloads from. */
#define HP_PAGE 4096
#define HP_MAX 200000
static struct {
    uintptr_t addr;
    uint64_t hash;
} g_hp[HP_MAX];
static volatile int g_nhp;

static unsigned char *g_hp_bytes; /* optional byte copy for diffs */

static void heap_snapshot(const char *tag)
{
    ranges_load();
    g_nhp = 0;
    unsigned char *buf = malloc(HP_PAGE);
    if (!buf)
        return;
    for (int r = 0; r < g_nrng && g_nhp < HP_MAX; ++r) {
        if (!g_rng[r].w)
            continue;
        if (g_rng[r].lo >= base && g_rng[r].hi <= base + 0x9000000)
            continue; /* the image, not the heap */
        if (g_rng[r].hi - g_rng[r].lo > 0x4000000)
            continue; /* huge audio buffers dwarf everything else */
        for (uintptr_t a = g_rng[r].lo; a + HP_PAGE <= g_rng[r].hi && g_nhp < HP_MAX;
             a += HP_PAGE) {
            if (!rd(a, buf, HP_PAGE))
                break;
            uint64_t h = 1469598103934665603ULL;
            for (int i = 0; i < HP_PAGE; ++i) {
                h ^= buf[i];
                h *= 1099511628211ULL;
            }
            g_hp[g_nhp].addr = a;
            g_hp[g_nhp].hash = h;
            g_nhp++;
        }
    }
    free(buf);
    mlog("mpcmini: heap snapshot '%s': %d pages\n", tag, g_nhp);
}

static void heap_diff(const char *tag)
{
    ranges_load();
    unsigned char *buf = malloc(HP_PAGE);
    if (!buf)
        return;
    int shown = 0;
    for (int i = 0; i < g_nhp && shown < 24; ++i) {
        if (!rd(g_hp[i].addr, buf, HP_PAGE))
            continue;
        uint64_t h = 1469598103934665603ULL;
        for (int k = 0; k < HP_PAGE; ++k) {
            h ^= buf[k];
            h *= 1099511628211ULL;
        }
        if (h != g_hp[i].hash) {
            mlog("mpcmini: changed page %p (%s)\n", (void *)g_hp[i].addr, tag);
            shown++;
        }
    }
    free(buf);
}

/* The three knobs' properties are consecutive objects 0x58 apart (K1 at
 * base+0x1ac0, K2 at +0x1b18, K3 at +0x1b70). Finding that triple identifies
 * the control surface without needing a knob move first. */
static int prop_like(uintptr_t x)
{
    uintptr_t head = 0;
    float v = 0;
    if (!rd(x + 0x10, &head, 8) || !head || !mapped((void *)head))
        return 0;
    if (!rd(x + 0x50, &v, 4) || !(v >= -2.0f && v <= 2.0f))
        return 0;
    return 1;
}

static int prop_listeners(uintptr_t x)
{
    uintptr_t head = 0;
    if (!rd(x + 0x10, &head, 8) || !head || head == x + 8)
        return 0;
    int n = 0;
    for (int i = 0; i < 32 && head && head != x + 8 && n < 99; ++i) {
        uintptr_t next = 0;
        n++;
        if (!rd(head + 8, &next, 8) || !next || !mapped((void *)next))
            break;
        head = next;
    }
    return n;
}

static uintptr_t find_knob_props(void)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t best = 0;
    int bestscore = 0;
    for (int r = 0; r < g_nrng; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 0xb0 + 0x58 + 8 <= n; o += 8) {
                uintptr_t cand = a + o;
                if (cand >= base && cand < base + 0x9000000)
                    continue;
                int ok = 1, score = 0;
                for (int k = 0; k < 3 && ok; ++k) {
                    size_t co = o + (size_t)k * 0x58;
                    uintptr_t head;
                    float v;
                    memcpy(&head, buf + co + 0x10, 8);
                    memcpy(&v, buf + co + 0x50, 4);
                    if (!head || !mapped((void *)head) || !(v >= -2.0f && v <= 2.0f))
                        ok = 0;
                    else if (head != cand + (uintptr_t)k * 0x58 + 8)
                        score++; /* a non-empty listener list */
                }
                if (ok && score > bestscore) {
                    bestscore = score;
                    best = cand;
                }
            }
        }
    }
    free(buf);
    if (best)
        mlog("mpcmini: knob triple %p score=%d\n", (void *)best, bestscore);
    return best;
}

/* Every listener of a real property has a vtable inside the app's
 * .data.rel.ro. If one does not, the property is not the one the page uses and
 * broadcasting through it would jump into garbage. */
static int prop_listeners_sane(uintptr_t P)
{
    uintptr_t node = 0;
    if (!rd(P + 0x10, &node, 8) || !node)
        return 0;
    int n = 0;
    for (; n < 32 && node && node != P + 8; ++n) {
        uintptr_t tgt = 0, next = 0, vt = 0;
        if (!rd(node + 0x18, &tgt, 8) || !rd(node + 8, &next, 8))
            return 0;
        tgt &= ~(uintptr_t)3;
        if (!tgt || !mapped((void *)tgt))
            return 0;
        if (!rd(tgt, &vt, 8))
            return 0;
        if (vt < base + 0x6ddf910 || vt > base + 0x7162018)
            return 0;
        if (!next || !mapped((void *)next))
            return 0;
        node = next;
    }
    return n > 0;
}

/* The trim parameter is the listener of the knob's property whose vtable is
 * TrimParameter's. That makes the Start/End pair exact, with no scanning and no
 * dependence on which knob was touched last. */
static uintptr_t param_from_prop(uintptr_t P)
{
    uintptr_t node = 0;
    if (!P || !rd(P + 0x10, &node, 8) || !node)
        return 0;
    for (int i = 0; i < 32 && node && node != P + 8; ++i) {
        uintptr_t tgt = 0, next = 0, vt = 0;
        if (!rd(node + 0x18, &tgt, 8) || !rd(node + 8, &next, 8))
            return 0;
        tgt &= ~(uintptr_t)3;
        if (tgt && mapped((void *)tgt) && rd(tgt, &vt, 8) && vt == base + 0x6e1b240)
            return tgt;
        if (!next || !mapped((void *)next))
            return 0;
        node = next;
    }
    return 0;
}

static void do_poke(void)
{
    if (g_poke_which >= 0) {
        int which = g_poke_which;
        g_poke_which = -1;
        mlog("mpcmini: poke fire %d v=%.3f\n", which, (double)g_poke_value);
        uintptr_t pp = prop_of_param((uintptr_t)g_trim_param0);
        if (!pp)
            pp = prop_of_param((uintptr_t)g_trim_param1);
        if (!pp) {
            uintptr_t kp = find_knob_props();
            if (kp)
                pp = kp;
        }
        mlog("mpcmini: poke prop=%p\n", (void *)pp);
        if (pp) {
            uintptr_t node = 0;
            if (rd(pp + 0x10, &node, 8) && node) {
                for (int i = 0; i < which && node && node != pp + 8; ++i) {
                    uintptr_t next = 0;
                    if (!rd(node + 8, &next, 8))
                        break;
                    node = next;
                }
                uintptr_t tgt = 0, vt = 0, slot = 0;
                if (rd(node + 0x18, &tgt, 8) && rd(tgt & ~(uintptr_t)3, &vt, 8) &&
                    rd(vt + 0x28, &slot, 8)) {
                    uintptr_t t = tgt & ~(uintptr_t)3;
                    mlog("mpcmini: poke %d target=%p slot=%p v=%.4f\n", which, (void *)t,
                         (void *)slot, (double)g_poke_value);
                    ((void (*)(float, void *))slot)(g_poke_value, (void *)t);
                    mlog("mpcmini: poke done\n");
                }
            }
        }
    }
}

static void commit_trim(void)
{
    void *p0 = g_trim_param0, *p1 = g_trim_param1;
    float v0 = g_trim_v0, v1 = g_trim_v1;
    if (!p0 || !p1)
        return;

    /* The Trim knobs' own effect, in their own order: the core (which stores
     * the value and the applied value on the parameter), the layer frames the
     * engine plays, then a repaint. Everything else tried so far - page cache
     * fields, slider widgets, property broadcasts - was measured to be a no-op
     * or to corrupt the page, so none of it is attempted any more. */
    /* A learned property broadcasts exactly like a knob; the others are
     * written through the trim core. Broadcasting even one of them is what
     * makes the page repaint. */
    if (getenv("MPCMINI_NODEPROBE"))
        probe_node((uintptr_t)p0);

    /* A property seen broadcasting the parameter's *current* value is the one
     * that owns it. Broadcasting through it is the knob's own path: the
     * broadcaster stores the value and notifies every listener, which is the
     * step that repaints the page. */
    float cur0 = 0, cur1 = 0;
    rd((uintptr_t)p0 + T_VALUE, &cur0, 4);
    rd((uintptr_t)p1 + T_VALUE, &cur1, 4);
    uintptr_t pr0 = learned_prop((uintptr_t)p0);
    uintptr_t pr1 = learned_prop((uintptr_t)p1);
    if (!pr0)
        pr0 = broadcast_prop((uintptr_t)p0, cur0);
    if (!pr1)
        pr1 = broadcast_prop((uintptr_t)p1, cur1);
    if (!pr0)
        pr0 = prop_of_param((uintptr_t)p0);
    if (!pr1)
        pr1 = prop_of_param((uintptr_t)p1);
    mlog("mpcmini: props %p/%p (cur %.4f/%.4f, seen %d)\n", (void *)pr0,
         (void *)pr1, (double)cur0, (double)cur1, g_nbc);

    /* Exact pair from the knob properties, when we can find them. */
    {
        uintptr_t kp = find_knob_props();
        if (kp) {
            uintptr_t sp = param_from_prop(kp), ep = param_from_prop(kp + 0x58);
            mlog("mpcmini: params from props %p/%p (had %p/%p)\n", (void *)sp, (void *)ep,
                 g_trim_param0, g_trim_param1);
            if (sp && ep) {
                g_trim_param0 = (void *)sp;
                g_trim_param1 = (void *)ep;
                p0 = (void *)sp;
                p1 = (void *)ep;
            }
        }
    }

    /* Replaying the knob through the app's CC dispatcher gives the exact visual
     * result of a manual trim (verified pixel-identical), but it crashes the
     * app when called from inside the button dispatch, so it is deferred to the
     * paint hook (opt-in). */
    if (getenv("MPCMINI_CCINJECT") && !g_in_inject)
        g_pending_cc = 1;
    if (g_in_inject) {
        uintptr_t base = 0, obj = 0;
        if (pr0)
            base = pr0 - 0x1ac0;
        else if (pr1)
            base = pr1 - 0x1b18;
        else {
            uintptr_t kp = find_knob_props();
            if (kp) {
                base = kp - 0x1ac0;
                mlog("mpcmini: knob property triple at %p\n", (void *)kp);
            }
        }
        if (base) {
            /* Only trust a control base whose K1/K2 properties really exist and
             * have a terminated listener list; a wrong one crashes the app. */
            uintptr_t p1 = base + 0x1ac0;
            uintptr_t head = 0;
            float v = 0;
            int ok = rd(p1 + 0x10, &head, 8) && head && mapped((void *)head) &&
                     rd(p1 + 0x50, &v, 4) && v >= -2.0f && v <= 2.0f;
            if (ok) {
                uintptr_t node = head;
                int steps = 0;
                for (; steps < 64 && node && node != p1 + 8; ++steps) {
                    uintptr_t next = 0;
                    if (!rd(node + 8, &next, 8) || !next || !mapped((void *)next))
                        break;
                    node = next;
                }
                if (node != p1 + 8)
                    ok = 0; /* list does not terminate at the sentinel */
            }
            if (ok && !prop_listeners_sane(p1))
                ok = 0;
            if (ok && !prop_listeners_sane(base + 0x1b18))
                ok = 0;
            if (!ok) {
                mlog("mpcmini: control base %p rejected\n", (void *)base);
                base = 0;
            } else {
                obj = ctl_obj_for(base);
            }
        }
        mlog("mpcmini: cc base=%p obj=%p\n", (void *)base, (void *)obj);
        /* What exactly will be called? Log every listener and its call slot so
         * a bad pointer can be matched against the fault address. */
        if (obj) {
            for (int k = 0; k < 2; ++k) {
                uintptr_t P = base + 0x1ac0 + (uintptr_t)k * 0x58;
                uintptr_t node = 0;
                if (!rd(P + 0x10, &node, 8) || !node)
                    continue;
                for (int i = 0; i < 8 && node && node != P + 8; ++i) {
                    uintptr_t tgt = 0, next = 0, vt = 0, slot = 0;
                    if (!rd(node + 0x18, &tgt, 8) || !rd(node + 8, &next, 8))
                        break;
                    tgt &= ~(uintptr_t)3;
                    if (rd(tgt, &vt, 8) && rd(vt + 0x28, &slot, 8))
                        mlog("mpcmini: P%p listener %p vt=%p slot28=%p\n", (void *)P,
                             (void *)tgt, (void *)vt, (void *)slot);
                    if (!next || !mapped((void *)next))
                        break;
                    node = next;
                }
            }
        }
        if (obj) {
            unsigned r0 = (unsigned)(v0 * 127.0f + 0.5f);
            unsigned r1 = (unsigned)(v1 * 127.0f + 0.5f);
            if (g_cc_ev_valid) {
                cc_replay(0x10, r0);
                cc_replay(0x11, r1);
            } else {
                cc_inject(obj, 0x10, r0);
                cc_inject(obj, 0x11, r1);
            }
            mlog("mpcmini: injected CC 0x10=%u 0x11=%u\n", r0, r1);
            {
                float c0 = -9, c1 = -9;
                if (pr0)
                    rd(pr0 + 0x50, &c0, 4);
                if (pr1)
                    rd(pr1 + 0x50, &c1, 4);
                mlog("mpcmini: app stored %.4f/%.4f (want %.4f/%.4f)\n", (double)c0,
                     (double)c1, (double)v0, (double)v1);
            }
            /* The page draws its trim only when it reloads the pad, so repeat
             * the pad press that is already in progress. */
            {
                int pd = g_last_pad;
                if (pd >= 0 && pd < 128 && g_pad_rec[pd].valid && orig_note_on) {
                    g_replaying_pad = 1;
                    orig_note_on(g_pad_rec[pd].input, g_pad_rec[pd].src,
                                 g_pad_rec[pd].pad, g_pad_rec[pd].props,
                                 g_pad_rec[pd].vel);
                    g_replaying_pad = 0;
                    mlog("mpcmini: re-selected pad %d to refresh the page\n", pd);
                }
            }
            /* Repaint the whole window, not just the page: the page is a
             * child and the app repaints from the top-level component. */
            if (g_screen) {
                uintptr_t root = (uintptr_t)g_screen;
                for (int i = 0; i < 8; ++i) {
                    uintptr_t parent = 0;
                    if (!rd(root + 0x18, &parent, 8) || !parent || !mapped((void *)parent))
                        break;
                    root = parent;
                }
                mlog("mpcmini: repaint root=%p (page %p)\n", (void *)root, g_screen);
                FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
                    (void *)root, 0, FIELD(root, 0x28, uint64_t));
                FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
                    g_screen, 0, FIELD(g_screen, 0x28, uint64_t));
            }
            return;
        }
    }
    /* Write the end first: the core clamps a start that would pass the current
     * end, so the pair must move outward before inward. */
    g_own_call = 1;
    if (pr1)
        FN(A_BROADCAST, void (*)(void *, float))((void *)pr1, v1);
    else
        FN(A_TRIM_CORE, void (*)(float, void *))(v1, p1);
    if (pr0)
        FN(A_BROADCAST, void (*)(void *, float))((void *)pr0, v0);
    else
        FN(A_TRIM_CORE, void (*)(float, void *))(v0, p0);
    /* Either parameter can reject a write while it still holds -1 (the core
     * treats that as "not yet bound"), so retry the pair a few times. */
    for (int attempt = 0; attempt < 4; ++attempt) {
        float c0 = 0, c1 = 0;
        int ok0 = rd((uintptr_t)p0 + T_VALUE, &c0, 4) && c0 > -0.5f;
        int ok1 = rd((uintptr_t)p1 + T_VALUE, &c1, 4) && c1 > -0.5f;
        if (ok0 && ok1)
            break;
        mlog("mpcmini: retry %d (p0=%.3f p1=%.3f)\n", attempt, (double)c0, (double)c1);
        if (!ok1)
            FN(A_TRIM_CORE, void (*)(float, void *))(v1, p1);
        if (!ok0)
            FN(A_TRIM_CORE, void (*)(float, void *))(v0, p0);
    }
    g_own_call = 0;

    if (g_trim_layer) {
        unsigned fa = (unsigned)(v0 * (float)g_trim_frames + 0.5f);
        unsigned fb = (unsigned)(v1 * (float)g_trim_frames + 0.5f);
        if (fb <= fa)
            fb = fa + 1;
        wr(g_trim_layer + 0x4d8, &fa, 4);
        wr(g_trim_layer + 0x588, &fa, 4);
        wr(g_trim_layer + 0x530, &fb, 4);
        mlog("mpcmini: layer %p <- %u..%u frames\n", (void *)g_trim_layer, fa, fb);
    }

    float a0 = v0, a1 = v1;
    wr((uintptr_t)p0 + T_APPLIED, &a0, 4);
    wr((uintptr_t)p1 + T_APPLIED, &a1, 4);

    float check = 0, check2 = 0;
    rd((uintptr_t)p0 + T_VALUE, &check, 4);
    rd((uintptr_t)p1 + T_VALUE, &check2, 4);
    g_mark_v0 = v0;
    g_mark_v1 = v1;
    mlog("mpcmini: commit core done p0=%.4f p1=%.4f (want %.4f/%.4f)\n",
         (double)check, (double)check2, (double)v0, (double)v1);

done:
    /* Re-press the last control so the page reloads: it only redraws its trim
     * when the pad is (re)selected. Off by default - the object recorded from a
     * press is not always the pad and pressing it can open a menu. */
    if (0 && g_last_press_obj && orig_buttons) {
        static unsigned char ev[32];
        memset(ev, 0, sizeof ev);
        *(uintptr_t *)ev = g_last_press_obj;
        ev[8] = 0x06;
        ev[9] = 0x7f;
        g_replaying_pad = 1;
        orig_buttons(ev);
        ev[9] = 0x00;
        orig_buttons(ev);
        g_replaying_pad = 0;
        mlog("mpcmini: re-pressed %p to refresh the page\n", (void *)g_last_press_obj);
    }
    if (g_screen)
        FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
            g_screen, 0, FIELD(g_screen, 0x28, uint64_t));
}


static void repaint_hook(void *comp, uint64_t a, uint64_t b)
{
    if (orig_repaint)
        orig_repaint(comp, a, b);
}

/* --------------------------------------------------------------- overlay */
/* Overdraw, the way am-mod does it: take the screen's vtable slot 27 (paint),
 * keep the stock function, and paint on top of it with the app's own Graphics
 * calls (setColour / fillRect / Font + drawText). */
#define A_SCREEN_VT 0x6e212a0
/* The Trim page is its own class (found by matching its cached percentages);
 * it is not the plain SampleModeScreen. */
#define A_TRIM_PAGE_VT 0x6e31ee8

static void (*orig_screen_paint)(void *, void *);

static void draw_colour(void *g, uint32_t v)
{
    FN(0x102d5e0, void (*)(void *, uint32_t))(g, v);
}
static void draw_rect(void *g, int x, int y, int w, int h)
{
    FN(0x102e000, void (*)(void *, int, int, int, int))(g, x, y, w, h);
}
static void draw_text(void *g, const char *s, int x, int y, int w, int h,
                      float size)
{
    uint64_t str = 0, font = 0;
    FN(0xf535c0, void (*)(uint64_t *, const char *))(&str, s);
    FN(0x1082810, void (*)(uint64_t *, int, float))(&font, 0, size);
    FN(0x102dca0, void (*)(void *, const uint64_t *))(g, &font);
    FN(0x10817b0, void (*)(void *, const uint64_t *, int, int, int, int, int, int,
                           float))(g, &str, x, y, w, h, 0x24, 1, 0.7f);
    FN(0x1070d60, void (*)(uint64_t *))(&font);
    FN(0xf0a290, void (*)(uint64_t *))(&str);
}

static void dump_sliders(void *p)
{
    uintptr_t want = base + 0x706e5d0;
    unsigned n = 0;
    for (long o = 0; o < 0x20000; o += 8) {
        uintptr_t q = 0;
        if (!rd((uintptr_t)p + o, &q, 8))
            break;
        if (q != want)
            continue;
        uintptr_t s = (uintptr_t)p + (uintptr_t)o;
        n++;
        mlog("mpcmini: slider +0x%lx\n", o);
        for (long k = 0x160; k < 0x230; k += 4) {
            float f = 0;
            if (!rd(s + k, &f, 4))
                break;
            if (f > -0.01f && f < 100.5f && f != 0.0f)
                mlog("mpcmini:   +0x%lx = %.4f\n", k, (double)f);
        }
    }
    mlog("mpcmini: dump done, %u sliders\n", n);
}

static void screen_paint(void *p, void *g)
{
    if (orig_screen_paint)
        orig_screen_paint(p, g);
    /* The knob replay runs here, on the UI thread at a safe point, instead of
     * inside the button dispatch where it corrupted the app's state. */
    if (g_pending_cc) {
        g_pending_cc = 0;
        g_in_inject = 1;
        commit_trim();
        g_in_inject = 0;
    }
    if (FIELD(p, 0, uintptr_t) != base + A_SCREEN_VT)
        return;
    if (g_screen != p) {
        g_screen = p;
        mlog("mpcmini: screen=%p\n", p);
    }
    {
        static int dumped;
        if (!dumped && getenv("MPCMINI_DUMP")) {
            dumped = 1;
            dump_sliders(p);
        }
    }
    /* Diff the painting screen between paints: whatever changes when a knob
     * turns is exactly what the page stores for its markers. */
    if (getenv("MPCMINI_SNAPDIFF")) {
        static unsigned char prev[0x8000], cur[0x8000];
        static uintptr_t prev_p;
        static size_t snap_len;
        static int have;
        if (prev_p != (uintptr_t)p) {
            size_t got = 0;
            for (size_t off = 0; off < sizeof prev; off += 0x1000) {
                if (!rd((uintptr_t)p + off, prev + off, 0x1000))
                    break;
                got += 0x1000;
            }
            if (got) {
                snap_len = got;
                prev_p = (uintptr_t)p;
                have = 1;
                mlog("mpcmini: snapdiff baseline %p (%zu bytes)\n", p, got);
            }
        } else if (have) {
            size_t got = 0;
            for (size_t off = 0; off < snap_len; off += 0x1000) {
                if (!rd((uintptr_t)p + off, cur + off, 0x1000))
                    break;
                got = off + 0x1000;
            }
            int shown = 0;
            for (size_t i = 0; i + 4 <= got && shown < 24; i += 4) {
                if (memcmp(cur + i, prev + i, 4) == 0)
                    continue;
                float a, b;
                memcpy(&a, prev + i, 4);
                memcpy(&b, cur + i, 4);
                mlog("mpcmini: diff +0x%zx %.4f -> %.4f\n", i, (double)a, (double)b);
                memcpy(prev + i, cur + i, 4);
                shown++;
            }
        }
    }
    /* Trim edges of the last autotrim, drawn where the page's own markers would
     * be: the page itself only re-reads its trim when the pad is re-selected, so
     * without this the result of SHIFT+B3 would be invisible. */
    if (g_mark_v0 >= 0.0f && g_mark_v1 >= 0.0f) {
        int x0 = 10 + (int)(g_mark_v0 * 295.0f + 0.5f);
        int x1 = 10 + (int)(g_mark_v1 * 295.0f + 0.5f);
        draw_colour(g, 0xffffffff);
        draw_rect(g, (uint64_t)x0, 88, 1, 98);
        draw_rect(g, (uint64_t)x1, 88, 1, 98);
        draw_rect(g, (uint64_t)x0, 88, 3, 1);
        draw_rect(g, (uint64_t)x1, 88, 3, 1);
    }
    if (!g_shift)
        return;
    /* third soft-key cell of the Sample page */
    draw_colour(g, 0xff000000);
    draw_rect(g, 213, 0, 107, 29);
    draw_colour(g, 0xffffffff);
    draw_text(g, "AutoTrim", 215, 1, 103, 22, 14.0f);
}

static int install_screen_paint(void)
{
    uintptr_t vtaddr = base + A_SCREEN_VT;
    if (!mapped((void *)vtaddr))
        return 0;
    uintptr_t *vt = (uintptr_t *)vtaddr;
    orig_screen_paint = (void (*)(void *, void *))vt[27];
    if (!orig_screen_paint)
        return 0;
    uintptr_t page = vtaddr & ~(uintptr_t)0xfff;
    if (mprotect((void *)page, 0x1000, PROT_READ | PROT_WRITE) != 0) {
        mlog("mpcmini: screen vtable mprotect failed: %s\n", strerror(errno));
        return 0;
    }
    vt[27] = (uintptr_t)screen_paint;
    mprotect((void *)page, 0x1000, PROT_READ);
    mlog("mpcmini: screen paint hooked (stock %p)\n", (void *)orig_screen_paint);
    return 1;
}

/* The soft-key row is rebuilt on SHIFT, and every rebuild assigns the label
 * strings through 0xf535c0(dest, text). While SHIFT is held we swap the B3
 * label ("Filter") for "AutoTrim" and remember its destination so the release
 * can put it back. */
static void *g_filter_dest;

/* The soft-key labels are juce::Label objects whose text is only ever copied,
 * so the char* assign hook never sees "Filter". Label::getText (0x11bae00)
 * returns the live juce::String by reference; mutating that string in place
 * renames the key without touching the layout of anything else. */
static void *g_label_str;

static const char *string_text(void *str, char *buf, size_t n)
{
    uintptr_t h = 0;
    if (!str || !rd((uintptr_t)str, &h, 8) || !h)
        return 0;
    int len = 0;
    if (rd(h + 4, &len, 4) && len > 0 && len < 200) { /* {ref, len, chars[]} */
        if (rd(h + 8, buf, (size_t)len < n - 1 ? (size_t)len : n - 1)) {
            buf[len < (int)n ? len : (int)n - 1] = 0;
            int ok = 1;
            for (int i = 0; i < len; ++i)
                if ((unsigned char)buf[i] < 32) { ok = 0; break; }
            if (ok)
                return buf;
        }
    }
    uintptr_t data = 0;
    size_t l = 0;
    if (rd(h, &data, 8) && rd(h + 8, &l, 8) && data && l > 0 && l < 200) {
        if (rd(data, buf, l < n - 1 ? l : n - 1)) {
            buf[l < n ? l : n - 1] = 0;
            return buf;
        }
    }
    return 0;
}

/* The soft-key captions are not juce::Label (Label::getText is never called),
 * so find the live juce::String that holds "Filter" once, and rename it in
 * place with the app's own String::operator=(const char*). */
static void *g_filter_strs[6];
static int g_nfilter_strs;

static void find_filter_strings(void)
{
    if (g_nfilter_strs)
        return;
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return;
    char txt[64];
    for (int r = 0; r < g_nrng && g_nfilter_strs < 6; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && g_ntp < 256; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && g_nfilter_strs < 6; o += 8) {
                uintptr_t str = a + o;
                const char *t = string_text((void *)str, txt, sizeof txt);
                if (t && !strcmp(t, "Filter")) {
                    g_filter_strs[g_nfilter_strs++] = (void *)str;
                    mlog("mpcmini: filter string at %p\n", (void *)str);
                }
            }
        }
    }
    free(buf);
}

/* Scanning the heap on the UI thread froze the app (one /proc/self/mem open
 * per read), so the rename is done by a worker that only touches the cached
 * strings afterwards. */
static void set_filter_text(const char *t)
{
    (void)t;
    if (g_screen)
        FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(g_screen, 0,
                                                          FIELD(g_screen, 0x28, uint64_t));
}

static void str_assign_hook(void *dest, const char *text)
{
    static int seen;
    if (debug_mode && text && seen < 25) {
        seen++;
        mlog("mpcmini: label '%s' dest=%p\n", text, dest);
    }
    if (text && !strcmp(text, "Filter"))
        g_filter_dest = dest;
    if (g_shift && text && !strcmp(text, "Filter"))
        text = "AutoTrim";
    if (orig_str_assign)
        orig_str_assign(dest, text);
}



/* ------------------------------------------------------------ MIDI (feature 2) */
/* Chromatic play from an external controller: the app already receives MIDI on
 * its ALSA rawmidi node; we read the same node and trigger the pads with the
 * full chromatic range instead of the 16-levels quantisation. */
static int midi_fd = -1;

static void *midi_thread(void *arg)
{
    (void)arg;
    /* find the rawmidi node */
    for (int i = 0; i < 8 && midi_fd < 0; ++i) {
        char p[64];
        snprintf(p, sizeof p, "/dev/snd/midiC%dD0", i);
        int fd = open(p, O_RDONLY | O_NONBLOCK);
        if (fd >= 0)
            midi_fd = fd;
    }
    if (midi_fd < 0) {
        mlog("mpcmini: midi: no rawmidi node\n");
        return 0;
    }
    mlog("mpcmini: midi: listening on fd %d\n", midi_fd);
    unsigned char b[3];
    int have = 0;
    for (;;) {
        unsigned char c;
        ssize_t n = read(midi_fd, &c, 1);
        if (n != 1) {
            struct timespec ts = {.tv_sec = 0, .tv_nsec = 20 * 1000 * 1000};
            nanosleep(&ts, 0);
            continue;
        }
        if (c & 0x80) {
            b[0] = c;
            have = 1;
            continue;
        }
        if (!have)
            continue;
        b[1] = c;
        unsigned char st = b[0] & 0xf0;
        if (st == 0x90 || st == 0x80) {
            static unsigned char last[16];
            unsigned char note = b[1];
            (void)last;
            play_note((int)note, st == 0x90, 0.8f);
        }
    }
    return 0;
}

static void play_note(int pitch, int on, float velocity)
{
    if (!orig_note_on)
        return;
    /* pad index carries the chromatic step; the app maps it through the
     * 16-levels table, so offset it by the octave the user selected. */
    int pad = pitch % 16;
    if (pad < 0)
        pad += 16;
    if (on)
        orig_note_on(0, 0, pad, 0, velocity);
}

/* -------------------------------------------------------- probe threads */
static void *needle_thread(void *arg)
{
    (void)arg;
    return 0;
}
static void *layer_needle(void *arg)
{
    (void)arg;
    return 0;
}

/* -------------------------------------------------- probe bookkeeping */
static void *g_setter[32];
static long g_setter_val[32];
static long g_setter_seq[32];
static long g_seq;
static int g_nsetter;
static unsigned char g_ev[16];
static volatile int g_have_ev;
static struct {
    void *val2;
} g_sl[64];
static int g_nsl;
/* Trim the sample the Sample page is showing to its silent edges.
 *
 * Everything is taken from the app: the length from the live TrimParameter
 * (+0x1e8), the sample from the layer chain, and the audio from the app's own
 * loaded wav. The write uses the app's own trim core
 * (0x15e70c0(float value, TrimParameter *param)) plus the frame-level state
 * the knobs keep (boundary object and layer). */
static void do_autotrim(void)
{
    g_ntp = 0;
    scan_trim_params();

    /* Keep only the parameters that are actually wired to the page: their
     * context must resolve to an instrument whose layer holds a sample with
     * exactly the parameter's length. That is what makes them live. */
    int good[16], ngood = 0;
    unsigned best_len = 0;
    for (int i = 0; i < g_ntp && ngood < 16; ++i) {
        if (g_tp[i].mx < 1000)
            continue;
        uintptr_t l = 0, ins = 0;
        if (!page_layers((uintptr_t)g_tp[i].p, &l, &ins))
            continue;
        uintptr_t smp = 0;
        if (!rd(l + 0x4a8, &smp, 8) || !smp)
            continue;
        if (frames_of_sample(smp) != g_tp[i].mx)
            continue;
        good[ngood++] = i;
        if (g_tp[i].mx > best_len)
            best_len = g_tp[i].mx;
    }
    mlog("mpcmini: autotrim: %d/%d params are wired (len=%u)\n", ngood, g_ntp,
         best_len);
    if (ngood < 2) {
        mlog("mpcmini: autotrim: no wired trim parameters\n");
        return;
    }
    int idx[8], nidx = 0;
    for (int pass = 1; pass >= 0 && nidx < 2; --pass) {
        nidx = 0;
        for (int k = 0; k < ngood && nidx < 8; ++k) {
            if (g_tp[good[k]].mx != best_len)
                continue;
            if (pass && g_tp[good[k]].val < 0.0f)
                continue;
            idx[nidx++] = good[k];
        }
    }
    if (nidx < 2) {
        mlog("mpcmini: autotrim: need two wired params, have %d\n", nidx);
        return;
    }
    unsigned N = best_len;
    uintptr_t p_start = (uintptr_t)g_tp[idx[0]].p;
    uintptr_t p_end = (uintptr_t)g_tp[idx[1]].p;

    /* If the knobs have been used, their parameters are the live ones: prefer
     * the learned one and the instance nearest to it (the stale copies sit at
     * the same address minus/plus a small offset). */
    /* The page initialises the parameters it drives; stale copies of the same
     * class keep -1. If two are initialised they are the live Start/End pair -
     * order them by value. */
    {
        uintptr_t init[8];
        float iv[8];
        int ni = 0;
        for (int i = 0; i < g_ntp && ni < 8; ++i) {
            if (g_tp[i].mx != N)
                continue;
            float v = 0;
            if (!rd((uintptr_t)g_tp[i].p + T_VALUE, &v, 4))
                continue;
            if (v < -0.5f || v > 1.5f)
                continue;
            init[ni] = (uintptr_t)g_tp[i].p;
            iv[ni] = v;
            ni++;
        }
        mlog("mpcmini: %d initialised params of %u\n", ni, N);
        for (int i = 0; i < g_nlis; ++i) {
            int found = 0;
            for (int k = 0; k < g_ntp; ++k)
                if ((uintptr_t)g_tp[k].p == g_lis[i].param)
                    found = 1;
            float v = 0;
            rd(g_lis[i].param + T_VALUE, &v, 4);
            mlog("mpcmini: knob param %p in scan=%d val=%.4f\n", (void *)g_lis[i].param,
                 found, (double)v);
        }
        if (ni >= 2) {
            for (int a = 0; a < ni; ++a)
                for (int b = a + 1; b < ni; ++b)
                    if (iv[b] < iv[a]) {
                        float t = iv[a];
                        iv[a] = iv[b];
                        iv[b] = t;
                        uintptr_t u = init[a];
                        init[a] = init[b];
                        init[b] = u;
                    }
            p_start = init[0];
            p_end = init[ni - 1];
            mlog("mpcmini: live pair %p(%.3f)/%p(%.3f)\n", (void *)p_start,
                 (double)iv[0], (void *)p_end, (double)iv[ni - 1]);
        }
    }
    if (g_npm > 0) {
        uintptr_t live = g_pm[0].param, other = 0;
        for (int i = 1; i < g_npm; ++i)
            if (g_pm[i].param != live) {
                other = g_pm[i].param;
                break;
            }
        if (!other) {
            long bestd = 1 << 30;
            for (int i = 0; i < g_ntp; ++i) {
                uintptr_t pp = (uintptr_t)g_tp[i].p;
                if (pp == live || g_tp[i].mx != N)
                    continue;
                long d = (long)pp - (long)live;
                if (d < 0)
                    d = -d;
                if (d < bestd) {
                    bestd = d;
                    other = pp;
                }
            }
        }
        if (other && other != live) {
            /* Start is the smaller of the two current values. */
            float va = 0, vb = 0;
            rd(live + T_VALUE, &va, 4);
            rd(other + T_VALUE, &vb, 4);
            if (vb < va) {
                uintptr_t t = live;
                live = other;
                other = t;
            }
            p_start = live;
            p_end = other;
            mlog("mpcmini: using learned params %p/%p\n", (void *)p_start,
                 (void *)p_end);
        }
    }

    /* Group the wired parameters by their context (*(param+0xb0)): the two
     * (Start/End) or three (..Loop) that belong to the displayed pad share it,
     * and the page's own pair is the group the Trim knobs write. Groups whose
     * context lives inside the heap near the parameter are stale copies. */
    {
        uintptr_t ctxs[8];
        int cnt[8], ngrp = 0;
        for (int k = 0; k < ngood; ++k) {
            if (g_tp[good[k]].mx != best_len)
                continue;
            uintptr_t c = 0;
            rd((uintptr_t)g_tp[good[k]].p + T_LAYER, &c, 8);
            int g = -1;
            for (int j = 0; j < ngrp; ++j)
                if (ctxs[j] == c) {
                    g = j;
                    break;
                }
            if (g < 0 && ngrp < 8) {
                g = ngrp++;
                ctxs[g] = c;
                cnt[g] = 0;
            }
            if (g >= 0)
                cnt[g]++;
        }
        int best = -1;
        for (int j = 0; j < ngrp; ++j)
            if (cnt[j] >= 2 && (best < 0 || cnt[j] > cnt[best]))
                best = j;
        if (best >= 0) {
            uintptr_t picks[8];
            int np = 0;
            for (int k = 0; k < ngood && np < 8; ++k) {
                uintptr_t c = 0;
                if (g_tp[good[k]].mx != best_len)
                    continue;
                rd((uintptr_t)g_tp[good[k]].p + T_LAYER, &c, 8);
                if (c == ctxs[best])
                    picks[np++] = (uintptr_t)g_tp[good[k]].p;
            }
            for (int i = 0; i < np; ++i)
                for (int j = i + 1; j < np; ++j) {
                    float vi = 0, vj = 0;
                    rd(picks[i] + T_VALUE, &vi, 4);
                    rd(picks[j] + T_VALUE, &vj, 4);
                    if (vj < vi) {
                        uintptr_t t = picks[i];
                        picks[i] = picks[j];
                        picks[j] = t;
                    }
                }
            p_start = picks[0];
            p_end = picks[1];
            {
                uintptr_t pg = (uintptr_t)find_sample_screen();
                for (int k = 0; k < ngood; ++k) {
                    if (g_tp[good[k]].mx != best_len)
                        continue;
                    uintptr_t pp = (uintptr_t)g_tp[good[k]].p;
                    float v = 0, ap = 0;
                    uintptr_t c = 0, m = 0;
                    rd(pp + T_VALUE, &v, 4);
                    rd(pp + T_APPLIED, &ap, 4);
                    rd(pp + T_LAYER, &c, 8);
                    rd(pp + T_MODEL, &m, 8);
                    long d = (long)m - (long)pg;
                    mlog("mpcmini: cand %p v=%.3f ap=%.3f ctx=%p model=%p d=%ld\n",
                         (void *)pp, (double)v, (double)ap, (void *)c, (void *)m, d);
                }
                mlog("mpcmini: page for deltas: %p\n", (void *)pg);
            }
            mlog("mpcmini: ctx group %p has %d params -> %p/%p\n", (void *)ctxs[best],
                 np, (void *)p_start, (void *)p_end);
        } else {
            mlog("mpcmini: no ctx group with 2+ params (groups=%d)\n", ngrp);
        }
    }

    /* The page object itself points at the parameters it drives. Several pads
     * can share a sample length, so length alone cannot tell which instance
     * the Trim knobs write - this can: scan the screen for our candidates. */
    {
        uintptr_t page = (uintptr_t)find_sample_screen();
        mlog("mpcmini: pagebind page=%p ngood=%d best_len=%u\n", (void *)page, ngood,
             best_len);
        for (int i = 0; i < g_ntp && i < 12; ++i) {
            float v = 0, ap = 0;
            uintptr_t l = 0, ins = 0;
            int ok = page_layers((uintptr_t)g_tp[i].p, &l, &ins);
            rd((uintptr_t)g_tp[i].p + T_VALUE, &v, 4);
            rd((uintptr_t)g_tp[i].p + T_APPLIED, &ap, 4);
            mlog("mpcmini: tp[%d] %p val=%.4f applied=%.3f len=%u layer=%p\n", i,
                 g_tp[i].p, (double)v, (double)ap, g_tp[i].mx,
                 ok ? (void *)(l) : 0);
        }
        if (page) {
            uintptr_t bound[8];
            int nb = 0;
            for (int i = 0; i < ngood && nb < 8; ++i) {
                uintptr_t cand = (uintptr_t)g_tp[good[i]].p;
                if (g_tp[good[i]].mx != best_len)
                    continue;
                for (long off = 0; off < 0x9000; off += 8) {
                    uintptr_t q = 0;
                    if (!rd(page + off, &q, 8))
                        break;
                    if (q == cand) {
                        bound[nb++] = cand;
                        mlog("mpcmini: page %p references param %p (+0x%lx)\n",
                             (void *)page, (void *)cand, off);
                        break;
                    }
                }
            }
            if (nb >= 2) {
                /* Start is the lower value, End the higher one. */
                for (int i = 0; i < nb; ++i)
                    for (int j = i + 1; j < nb; ++j) {
                        float vi = 0, vj = 0;
                        rd(bound[i] + T_VALUE, &vi, 4);
                        rd(bound[j] + T_VALUE, &vj, 4);
                        if (vj < vi) {
                            uintptr_t t = bound[i];
                            bound[i] = bound[j];
                            bound[j] = t;
                        }
                    }
                p_start = bound[0];
                p_end = bound[nb - 1];
                mlog("mpcmini: page-bound params %p/%p (of %d)\n", (void *)p_start,
                     (void *)p_end, nb);
            } else {
                mlog("mpcmini: page references %d of our params, keeping guess\n", nb);
            }
        }
    }

    uintptr_t layers = 0, instr = 0;
    uintptr_t sample = 0;
    unsigned a = 0, b = 0;
    char path[512] = {0};
    if (!page_layers(p_start, &layers, &instr)) {
        mlog("mpcmini: autotrim: no layer chain\n");
        return;
    }
    /* Primary: the sample object the page is showing (it owns the .wav). */
    {
        unsigned dfr = 0;
        char dpath[512] = {0};
        if (find_displayed_sample(&dfr, dpath, sizeof dpath) && dfr == N &&
            dpath[0]) {
            snprintf(path, sizeof path, "%s", dpath);
            for (int i = 0; i < 128 && !sample; ++i) {
                uintptr_t s2 = 0;
                if (rd(layers + (uintptr_t)i * 0xb30 + 0x4a8, &s2, 8) &&
                    s2 && frames_of_sample(s2) == N)
                    sample = s2;
            }
        }
    }
    /* The core reads layer 0 of the page instrument, so use that sample. */
    if (!path[0]) {
        uintptr_t s0 = 0;
        if (rd(layers + 0x4a8, &s0, 8) && s0 && frames_of_sample(s0) == N &&
            wav_path_of(s0, N, path, sizeof path)) {
            sample = s0;
            mlog("mpcmini: autotrim: pad0 sample=%p path='%s'\n", (void *)s0, path);
        }
    }
    /* Fallback: any layer whose sample has N frames and whose Sample object
     * carries a matching .wav. */
    if (!path[0]) {
        uintptr_t cand[16];
        int ncand = 0, prefer = -1;
        for (int pad = 0; pad < 128 && ncand < 16; ++pad) {
            uintptr_t s = 0;
            if (!rd(layers + (uintptr_t)pad * 0xb30 + 0x4a8, &s, 8) || !s)
                continue;
            if (frames_of_sample(s) != N)
                continue;
            if (pad == g_last_pad)
                prefer = ncand;
            cand[ncand++] = s;
        }
        if (prefer > 0) {
            uintptr_t t = cand[0];
            cand[0] = cand[prefer];
            cand[prefer] = t;
        }
        for (int i = 0; i < ncand && !path[0]; ++i)
            if (wav_path_of(cand[i], N, path, sizeof path))
                sample = cand[i];
        mlog("mpcmini: autotrim: candidates=%d sample=%p\n", ncand, (void *)sample);
    }
    if (!path[0])
        wav_by_frames(N, path, sizeof path);
    mlog("mpcmini: autotrim: sample=%p path='%s'\n", (void *)sample, path);

    /* Manual probe: MPCMINI_TESTTRIM="0.091,0.454" drives the write path with
     * fixed bounds so the commit mechanism can be tested on its own. */
    const char *tt = getenv("MPCMINI_TESTTRIM");
    if (tt && *tt && layers) {
        float x0 = 0.091f, x1 = 0.454f;
        sscanf(tt, "%f,%f", &x0, &x1);
        unsigned ca = 0, cb2 = 0;
        rd(layers + 0x4d8, &ca, 4);
        rd(layers + 0x530, &cb2, 4);
        g_trim_cur0 = (float)ca / (float)N;
        g_trim_cur1 = (float)cb2 / (float)N;
        g_trim_param0 = (void *)p_start;
        g_trim_param1 = (void *)p_end;
        g_trim_v0 = x0;
        g_trim_v1 = x1;
        g_trim_frames = N;
        g_trim_pending = 1;
        mlog("mpcmini: TESTTRIM %.4f..%.4f cur=%.4f/%.4f\n", (double)x0,
             (double)x1, (double)g_trim_cur0, (double)g_trim_cur1);
        return;
    }
    unsigned fr = 0, rt = 0, ch = 0, doff = 0, bps = 2;
    if (!path[0] || !wav_info(path, &fr, &rt, &ch, &doff, &bps) || fr != N) {
        mlog("mpcmini: autotrim: no verified wav for %u frames\n", N);
        return;
    }
    if (!wav_edges(path, fr, doff, ch, bps, &a, &b) || (a == 0 && b + 1 >= N)) {
        mlog("mpcmini: autotrim: nothing to trim\n");
        return;
    }
    mlog("mpcmini: autotrim: frames=%u ch=%u edges=%u..%u\n", fr, ch, a, b);

    float v0 = (float)a / (float)N;
    float v1 = (float)(b + 1) / (float)N;
    if (v1 <= v0 || v1 > 1.0f)
        v1 = 1.0f;
    mlog("mpcmini: autotrim frames=%u -> %.4f..%.4f p0=%p p1=%p\n", N, (double)v0,
         (double)v1, (void *)p_start, (void *)p_end);

    /* frame-level state the knobs keep (verified by diffing the layer) */
    if (sample && layers) {
        uintptr_t lay = layers;
        for (int pad = 0; pad < 128; ++pad) {
            uintptr_t s = 0;
            if (rd(layers + (uintptr_t)pad * 0xb30 + 0x4a8, &s, 8) && s == sample) {
                lay = layers + (uintptr_t)pad * 0xb30;
                break;
            }
        }
        unsigned ca = 0, cb = 0;
        rd(lay + 0x4d8, &ca, 4);
        rd(lay + 0x530, &cb, 4);
        g_trim_cur0 = (float)ca / (float)N;
        g_trim_cur1 = (float)cb / (float)N;
        unsigned sa = a, sb = b;
        wr(lay + 0x4d8, &sa, 4);
        wr(lay + 0x588, &sa, 4);
        wr(lay + 0x530, &sb, 4);
        mlog("mpcmini: layer %p <- %u..%u frames\n", (void *)lay, sa, sb);
    }
    g_trim_param0 = (void *)p_start;
    g_trim_param1 = (void *)p_end;
    g_trim_v0 = v0;
    g_trim_v1 = v1;
    g_trim_frames = N;
    g_trim_layer = layers;
    g_trim_screen = (uintptr_t)find_sample_screen();
    mlog("mpcmini: autotrim commit (message thread)\n");
    {
        char line[256];
        int n = 0;
        n += snprintf(line + n, sizeof line - n, "native bcast props:");
        for (int i = 0; i < g_nbp && n < (int)sizeof line - 24; ++i)
            n += snprintf(line + n, sizeof line - n, " %p", (void *)g_bp[i]);
        mlog("mpcmini: %s\n", line);
        n = 0;
        n += snprintf(line + n, sizeof line - n, "learned props:");
        for (int i = 0; i < g_npm && n < (int)sizeof line - 32; ++i)
            n += snprintf(line + n, sizeof line - n, " %p(p=%p)",
                          (void *)g_pm[i].prop, (void *)g_pm[i].param);
        mlog("mpcmini: %s\n", line);
    }
    commit_trim();
}


/* every SampleModeScreen instance in the heap (the page paints from one of
 * them, the Trim knobs may drive another) */
static int find_screens(uintptr_t *out, int max)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return 0;
    uintptr_t want = base + A_SCREEN_VT;
    int n = 0;
    for (int r = 0; r < g_nrng && n < max; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && n < max; a += 65536) {
            size_t sz = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            sz &= ~(size_t)7;
            if (!rd(a, buf, sz))
                break;
            for (size_t o = 0; o + 8 <= sz && n < max; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q == want)
                    out[n++] = a + o;
            }
        }
    }
    free(buf);
    return n;
}

/* Probe: /run/mpcmini.slider holds "N,V" - write percent V into the N-th
 * ui::Slider of the painting screen and repaint. Used to identify which slider
 * drives which locator. */
static void *slider_probe_thread(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 300 * 1000 * 1000};
    int last_n = -1;
    float last_v = -1;
    for (;;) {
        nanosleep(&ts, 0);
        int fd = open("/run/mpcmini.slider", O_RDONLY);
        if (fd < 0)
            continue;
        char buf[64];
        ssize_t n = read(fd, buf, sizeof buf - 1);
        close(fd);
        if (n <= 0)
            continue;
        buf[n] = 0;
        int idx = -1;
        float val = 0;
        if (sscanf(buf, "%d,%f", &idx, &val) != 2 || idx < 0)
            continue;
        if (idx == last_n && val == last_v)
            continue;
        last_n = idx;
        last_v = val;
        if (!g_screen)
            continue;
        uintptr_t want = base + 0x706e5d0, hit = 0;
        int k = 0;
        for (long o = 0; o < 0x20000; o += 8) {
            uintptr_t q = 0;
            if (!rd((uintptr_t)g_screen + o, &q, 8))
                break;
            if (q != want)
                continue;
            if (k++ == idx) {
                hit = (uintptr_t)g_screen + (uintptr_t)o;
                break;
            }
        }
        if (!hit)
            continue;
        wr(hit + 0x1b4, (void *)&val, 4);
        mlog("mpcmini: probe slider %d @%p = %.2f\n", idx, (void *)hit, (double)val);
        FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
            g_screen, 0, FIELD(g_screen, 0x28, uint64_t));
    }
    return 0;
}

/* Probe: /run/mpcmini.heap holds "snap" or "diff" for the heap fingerprint. */
static void *heap_probe_thread(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 400 * 1000 * 1000};
    for (;;) {
        nanosleep(&ts, 0);
        int fd = open("/run/mpcmini.heap", O_RDONLY);
        if (fd < 0)
            continue;
        char buf[32];
        ssize_t n = read(fd, buf, sizeof buf - 1);
        close(fd);
        unlink("/run/mpcmini.heap");
        if (n <= 0)
            continue;
        buf[n] = 0;
        if (!strncmp(buf, "snap", 4))
            heap_snapshot("mark");
        else if (!strncmp(buf, "diff", 4))
            heap_diff("after knob");
        else if (!strncmp(buf, "find ", 5))
            find_float_pair((float)atof(buf + 5));
        else if (!strncmp(buf, "poke ", 5)) {
            int w = atoi(buf + 5);
            const char *sp = strchr(buf + 5, ' ');
            g_poke_value = sp ? (float)atof(sp + 1) : 0.5f;
            g_poke_which = w;
        }
    }
    return 0;
}

/* Probe: find every place in writable memory where the float pair
 * (0.0, value) sits adjacent - the trim model would keep Start/End that way. */
static void find_float_pair(float v)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return;
    unsigned char pat[4];
    memcpy(pat, &v, 4);
    int shown = 0;
    for (int r = 0; r < g_nrng; ++r) {
        if (!g_rng[r].w)
            continue;
        if (g_rng[r].lo >= base && g_rng[r].hi <= base + 0x9000000)
            continue;
        if (g_rng[r].hi - g_rng[r].lo > 0x4000000)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)3;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 4; o + 4 <= n; ++o) {
                if (memcmp(buf + o, pat, 4))
                    continue;
                float prev;
                memcpy(&prev, buf + o - 4, 4);
                if (prev > -1e-4f && prev < 1e-4f && shown < 24) {
                    shown++;
                    mlog("mpcmini: pair at %p (prev %.4f next %.4f)\n",
                         (void *)(a + o - 4), (double)prev, (double)v);
                }
            }
        }
    }
    mlog("mpcmini: pair search done, %d hits\n", shown);
    free(buf);
}

/* Probe: /run/mpcmini.bcast holds "i,v" - broadcast g_bp[i] with value v, so we
 * can find which recorded property drives the page. */
static void *bcast_probe_thread(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 300 * 1000 * 1000};
    int last_i = -1;
    float last_v = -1;
    for (;;) {
        nanosleep(&ts, 0);
        int fd = open("/run/mpcmini.bcast", O_RDONLY);
        if (fd < 0)
            continue;
        char buf[64];
        ssize_t n = read(fd, buf, sizeof buf - 1);
        close(fd);
        if (n <= 0)
            continue;
        buf[n] = 0;
        int idx = -1;
        float val = 0;
        if (sscanf(buf, "%d,%f", &idx, &val) != 2 || idx < 0 || idx >= g_nbp)
            continue;
        if (idx == last_i && val == last_v)
            continue;
        last_i = idx;
        last_v = val;
        mlog("mpcmini: bcast probe %d (%p) = %.4f\n", idx, (void *)g_bp[idx],
             (double)val);
        FN(A_BROADCAST, void (*)(void *, float))((void *)g_bp[idx], val);
    }
    return 0;
}

/* Re-arms the software watchpoint from /run/mpcmini.watch (hex address), so the
 * address can be set after the app has started and picked a new heap. */
static void *watch_thread(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 200 * 1000 * 1000};
    for (;;) {
        nanosleep(&ts, 0);
        /* the app installs its own crash handler after us; keep ours on top */
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = fault_handler;
        sa.sa_flags = SA_SIGINFO;
        sigemptyset(&sa.sa_mask);
        sigaction(SIGSEGV, &sa, 0);
        int fd = open("/run/mpcmini.watch", O_RDONLY);
        if (fd < 0)
            continue;
        char buf[64];
        ssize_t n = read(fd, buf, sizeof buf - 1);
        close(fd);
        if (n <= 0)
            continue;
        buf[n] = 0;
        if (!strncmp(buf, "scan", 4)) {
            uintptr_t sc[8];
            int ns = find_screens(sc, 8);
            mlog("mpcmini: %d screen instance(s)\n", ns);
            for (int i = 0; i < ns; ++i) {
                mlog("mpcmini:   screen[%d]=%p\n", i, (void *)sc[i]);
                watch_arm(sc[i] + 0x1f4c, "screen");
                watch_arm(sc[i] + 0x72c8, "screenfrac");
            }
            unlink("/run/mpcmini.watch");
            continue;
        }
        uintptr_t a = (uintptr_t)strtoull(buf, 0, 16);
        if (a)
            watch_arm(a, "addr");
    }
    return 0;
}

static void *trim_worker(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 100 * 1000 * 1000};
    for (;;) {
        nanosleep(&ts, 0);
        if (g_trim_req) {
            g_trim_req = 0;
            do_autotrim();
        }
    }
    return 0;
}

static int find_main(struct dl_phdr_info *info, size_t size, void *out)
{
    (void)size;
    mlog("mpcmini: entry '%s' addr=%p phnum=%u\n", info->dlpi_name ? info->dlpi_name : "",
         (void *)info->dlpi_addr, (unsigned)info->dlpi_phnum);
    for (unsigned i = 0; i < info->dlpi_phnum; ++i) {
        const ElfW(Phdr) *p = &info->dlpi_phdr[i];
        if (p->p_type == PT_LOAD)
            mlog("mpcmini:   PT_LOAD vaddr=%p memsz=%p filesz=%p\n",
                 (void *)p->p_vaddr, (void *)p->p_memsz, (void *)p->p_filesz);
    }
    if (info->dlpi_name && *info->dlpi_name)
        return 0;
    for (unsigned i = 0; i < info->dlpi_phnum; ++i) {
        const ElfW(Phdr) *p = &info->dlpi_phdr[i];
        /* The stock app is the only PT_LOAD whose mapped range covers .text. */
        if (p->p_type == PT_LOAD && 0xb55b00u >= p->p_vaddr &&
            0xb55b00u < p->p_vaddr + p->p_memsz) {
            *(uintptr_t *)out = info->dlpi_addr;
            return 1;
        }
    }
    return 0;
}

__attribute__((constructor)) static void load(void)
{
    log_fd = open("/run/mpcmini.log", O_WRONLY | O_CREAT | O_APPEND, 0644);
    {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = fault_handler;
        sa.sa_flags = SA_SIGINFO;
        sigemptyset(&sa.sa_mask);
        sigaction(SIGSEGV, &sa, 0);
        sigaction(SIGBUS, &sa, 0);
        sigaction(SIGILL, &sa, 0);
        sigaction(SIGABRT, &sa, 0);
    }
    uintptr_t found = 0;
    if (!dl_iterate_phdr(find_main, &found)) {
        mlog("mpcmini: stock app not found; not installing\n");
        return;
    }
    base = found;

    /* The B at each site reaches only +-128 MB, so the relay must live near
     * the app. The stock binary ends at ~0x71b0000; try just above it. */
    for (uintptr_t off = 0x7900000; off < 0x7f00000; off += 0x10000) {
        relay_page = mmap((void *)(base + off), 4096, PROT_READ | PROT_WRITE,
                          MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
        if (relay_page != MAP_FAILED)
            break;
    }
    if (relay_page == MAP_FAILED) {
        mlog("mpcmini: relay mmap near base failed\n");
        return;
    }
    /* trampolines: the displaced instruction plus a branch back into the app */
    for (uintptr_t off = 0x7920000; off < 0x7f80000; off += 0x10000) {
        tramp_page = mmap((void *)(base + off), 4096, PROT_READ | PROT_WRITE,
                          MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
        if (tramp_page != MAP_FAILED)
            break;
    }
    if (tramp_page == MAP_FAILED) {
        mlog("mpcmini: trampoline mmap near base failed\n");
        return;
    }
    mlog("mpcmini: relay at %p (site %p)\n", relay_page, (void *)(base + A_SIXTEEN_PROC));
    const char *env = getenv("MPCMINI_HOOKS");
    if (env && *env)
        hook_mask = atoi(env);
    const char *safe_env = getenv("MPCMINI_SAFE");
    const char *dbg_env = getenv("MPCMINI_DEBUG");
    safe_mode = safe_env && atoi(safe_env) != 0;
    debug_mode = dbg_env && atoi(dbg_env) != 0;
    const char *probe_env = getenv("MPCMINI_PROBE");
    probe_mode = probe_env && atoi(probe_env) != 0;
    const char *bits_env = getenv("MPCMINI_PROBES");
    if (bits_env && *bits_env)
        probe_bits = atoi(bits_env);
    void *t_ctor = (hook_mask & 1) ? install_hook(base + A_SIXTEEN_CTOR, (void *)sixteen_ctor) : 0;
    void *t_proc = (hook_mask & 2) ? install_hook(base + A_SIXTEEN_PROC, (void *)sixteen_proc) : 0;
    void *t_cc = install_hook(base + 0x159f230, (void *)cc_capture);
    if (t_cc)
        orig_cc = (void (*)(void *, unsigned, float, float))t_cc;
    void *t_loc = install_hook(base + A_LOC_BOUNDS, (void *)loc_bounds);
    if (t_loc)
        orig_loc_bounds = (void (*)(void *))t_loc;
    void *t_poff = install_hook(base + A_PITCH_OFF, (void *)pitch_off);
    if (t_poff)
        orig_pitch_off = (void (*)(void *, uint64_t, uint64_t, int))t_poff;
    void *t_btn = (hook_mask & 16) ? install_hook(base + A_BUTTONS, (void *)buttons) : 0;
    void *t_note = (hook_mask & 32) ? install_hook(base + A_NOTE_ON, (void *)note_on) : 0;
    void *t_cs = (hook_mask & 64) ? install_hook(base + A_CS_EVENT, (void *)cs_event) : 0;
    void *t_off = (hook_mask & 128) ? install_hook(base + A_NOTE_OFF, (void *)note_off) : 0;
    void *t_trim = install_hook(base + A_TRIM_CORE, (void *)trim_probe); /* also learns param->property */
    void *t_bc = install_hook(base + A_BROADCAST, (void *)bcast_hook);
    if (t_bc)
        orig_bcast = (void (*)(void *, float))t_bc;
    void *t_rp = install_hook(base + A_REPAINT, (void *)repaint_hook);
    if (t_rp)
        orig_repaint = (void (*)(void *, uint64_t, uint64_t))t_rp;
    void *t_obs = install_hook(base + A_OBS_SET, (void *)obs_set);
    if (t_obs)
        orig_obs_set = (void (*)(void *, float))t_obs;
    void *t_sa = install_hook(base + A_STR_ASSIGN, (void *)str_assign_hook);

    void *t_lt = 0;   /* Label::getText hook removed: wrong ABI, crashed the sample browser */
    void *t_dt = 0;   /* drawText overlay removed: the third slot is never drawn through it */
    if (t_ctor)
        orig_sixteen_ctor = (void *(*)(void *, void *, void *, void *))t_ctor;
    if (t_proc)
        orig_sixteen_proc = (void (*)(void *, int, void *))t_proc;
    if (t_btn)
        orig_buttons = (void (*)(void *))t_btn;
    if (t_note)
        orig_note_on = (void (*)(void *, int, int, void *, float))t_note;
    if (t_cs)
        orig_cs = (void (*)(void *, int, void *))t_cs;
    if (t_off)
        orig_note_off = (void (*)(void *, int, int))t_off;
    if (t_trim)
        orig_trim = (void (*)(void *, float))t_trim;
    if (t_sa)
        orig_str_assign = (void (*)(void *, const char *))t_sa;
    if (t_lt)
        orig_label_text = (void *(*)(void *))t_lt;
    if (t_dt)
        orig_draw_text = (void (*)(void *, const uint64_t *, int, int, int, int, int, int, float))t_dt;
    install_screen_paint();
    {
        pthread_t wth;
        pthread_create(&wth, 0, watch_thread, 0);
        pthread_t sth;
        pthread_create(&sth, 0, slider_probe_thread, 0);
        pthread_t bth;
        pthread_create(&bth, 0, bcast_probe_thread, 0);
        pthread_t hth;
        pthread_create(&hth, 0, heap_probe_thread, 0);
    }
    if (!relay_used) {
        mlog("mpcmini: no hook installed\n");
        return;
    }
    if (mprotect(relay_page, 4096, PROT_READ | PROT_EXEC) != 0)
        _exit(121);
    if (mprotect(tramp_page, 4096, PROT_READ | PROT_EXEC) != 0)
        _exit(122);
    mlog("mpcmini: codes relay=%p tramp=%p\n", (void *)relay_page,
         (void *)tramp_page);
    pthread_t th;
    /* External MIDI is on unless explicitly disabled. */
    /* The app owns the control-surface rawmidi node; opening it here makes the
     * stock app fail to start, so the MIDI feature stays opt-in. */
    const char *midi_env = getenv("MPCMINI_MIDI");
    if (midi_env && atoi(midi_env) != 0)
        pthread_create(&th, 0, midi_thread, 0);
    {
        pthread_t tw;
        pthread_create(&tw, 0, trim_worker, 0);
    }
    mlog("mpcmini: base=%p mask=%d safe=%d probe=%d (ctor=%d proc=%d plus=%d minus=%d)\n",
         (void *)base, hook_mask, safe_mode, probe_mode, t_ctor != 0, t_proc != 0);
}
