/* minimod — a minimal LD_PRELOAD mod for the Akai MPC Sample (firmware 1.3.0.12).
 *
 * It never touches the firmware on disk: the SD autorun script preloads this
 * library into the stock app, we find the app's load base and install a few
 * in-memory branch hooks (the same technique am-mod documents).
 *
 * Features
 *   1. octave shift in 16 Levels: PLUS / MINUS move the whole chromatic row by
 *      an octave while 16 LEVELS is lit.
 *   2. chromatic external MIDI playback for the selected sample in 16 Levels.
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
#include <setjmp.h>
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
/* Release switches. */
#ifndef MINIMOD_LOG
#define MINIMOD_LOG 0      /* 0: no log file, no output at all */
#endif
/* Release builds carry no probes: the /run channels, the experimental hooks
 * and the polling threads are all compiled out unless MINIMOD_DEBUG is set. */
#ifndef MINIMOD_DEBUG
#define MINIMOD_DEBUG 0
#endif

#define A_SIXTEEN_CTOR 0x1545a50   /* 16 Levels screen constructor        */
#define A_SIXTEEN_PROC 0x21472f0   /* 16 Levels pitch computation         */
#define A_BUTTONS 0x159f2f0        /* physical button dispatch (event)    */
#define A_NOTE_ON 0x1e27360        /* note on (input, src, pad, props, v) */
#define A_NOTE_OFF 0x1e270d0       /* note off (input, src, pad)           */
#define A_MIDI_NOTE_EVENT 0x211b090 /* native MIDI note-event dispatcher     */
#define A_BASS_AUDIO_WRITER 0x03f398f0 /* Ghidra 0x040398f0; planar float -> PCM */
#define A_CS_EVENT 0x23aff60       /* control-surface event (self, c, p)  */
#define A_TRIM_WRITE 0x14d0890     /* generic observable property setter  */
#define A_SCREEN_RESIZED 0x1660370 /* screen resize notification          */
#define A_SAMPLE_FRAMES 0x2941f80  /* sample frame count getter           */
#define A_PROG_INST 0x286bbe0      /* program -> instrument accessor      */
#define A_DATA_SIGNAL 0x140dbc0    /* signed integer event emitter, Ghidra 0x150dbc0 */
#define A_REPAINT 0x1149880        /* component repaint (comp, x, y)      */
#define A_TRIM_CORE 0x15e70c0      /* trim core (float value, param)      */
#define A_BROADCAST 0x13bb930      /* value broadcaster (property, float) */
#define A_TRIM_REG 0x16b6060       /* waveform locator bounds (probe)     */
#define A_STR_ASSIGN 0xf535c0      /* juce::String assign (dest, text)    */
#define A_SCREEN_VT 0x6e212a0      /* SampleModeScreen vtable              */
#define A_OBS_SET 0x14a7580        /* observable setter (float, property) */
#define A_LABEL_TEXT 0x11bae00     /* Label::getText (returns String&)    */

#include "hpss.h"
#include "spectral.h"
#include "h3m_effect.h"
#include "bass_sample.h"

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

static volatile int g_cc_recover;
static sigjmp_buf g_cc_jmp;

static void mlog(const char *fmt, ...);
static void toast(const char *text);
static long long now_ms(void);
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
        mlog("minimod: watch(%s) %p page=%p\n", why, (void *)addr, (void *)page);
    } else {
        mlog("minimod: watch(%s) %p page=%p failed: %s\n", why, (void *)addr,
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
        mlog("minimod: WATCH addr=%p pc=%p lr=%p sp=%p\n", si->si_addr, (void *)pc,
             (void *)lr, (void *)sp);
        mprotect((void *)((uintptr_t)si->si_addr & ~(uintptr_t)0xfff), 0x1000,
                 PROT_READ | PROT_WRITE);
        for (int i = 0; i < g_nwpage; ++i)
            if (g_wpages[i] == ((uintptr_t)si->si_addr & ~(uintptr_t)0xfff))
                g_wpages[i] = 0;
        return;
    }
    mlog("minimod: FAULT sig=%d addr=%p pc=%p lr=%p sp=%p\n", sig,
         si ? si->si_addr : 0, (void *)pc, (void *)lr, (void *)sp);
    /* The knob replay can end in a call through a bad pointer inside the app's
     * follow-up work. That jump faults before touching the stack, so jumping
     * back out of it is safe enough and keeps the app alive. */
    if (si && si->si_signo == SIGSEGV) {
        /* registers at the fault identify what the app was working on */
        ucontext_t *uc = (ucontext_t *)ucv;
        if (uc)
            mlog("minimod: fault regs x0=%llx x1=%llx x2=%llx x3=%llx x19=%llx x20=%llx "
                 "x21=%llx x22=%llx x23=%llx\n",
                 (unsigned long long)uc->uc_mcontext.regs[0],
                 (unsigned long long)uc->uc_mcontext.regs[1],
                 (unsigned long long)uc->uc_mcontext.regs[2],
                 (unsigned long long)uc->uc_mcontext.regs[3],
                 (unsigned long long)uc->uc_mcontext.regs[19],
                 (unsigned long long)uc->uc_mcontext.regs[20],
                 (unsigned long long)uc->uc_mcontext.regs[21],
                 (unsigned long long)uc->uc_mcontext.regs[22],
                 (unsigned long long)uc->uc_mcontext.regs[23]);
    }
    if (g_cc_recover) {
        g_cc_recover = 0;
        siglongjmp(g_cc_jmp, 1);
    }
    _exit(139);
    /* Never resume after a fatal signal: restore the default disposition and
     * let the app die the way it would without the mod. */
    {
        struct sigaction dfl;
        memset(&dfl, 0, sizeof dfl);
        dfl.sa_handler = SIG_DFL;
        sigemptyset(&dfl.sa_mask);
        sigaction(sig, &dfl, 0);
        raise(sig);
    }
}

static volatile int g_cc_recover;
static sigjmp_buf g_cc_jmp;

#define LOG_CAP (256u * 1024u)
static pthread_mutex_t g_log_mtx = PTHREAD_MUTEX_INITIALIZER;
static unsigned long g_log_bytes;
static int g_log_full;

static void mlog(const char *fmt, ...)
{
#if !MINIMOD_LOG
    (void)fmt;
#else
    if (log_fd < 0 || g_log_full)
        return;
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    if (n <= 0)
        return;
    /* vsnprintf returns the length the output *would* have had; clamp it or the
     * write below would read past buf. */
    if (n > (int)sizeof buf - 1)
        n = (int)sizeof buf - 1;

    pthread_mutex_lock(&g_log_mtx);
    if (g_log_bytes + (unsigned long)n > LOG_CAP) {
        static const char tail[] = "minimod: log cap reached, going quiet\n";
        (void)!write(log_fd, tail, sizeof tail - 1);
        g_log_full = 1;
    } else {
        (void)!write(log_fd, buf, (size_t)n);
        g_log_bytes += (unsigned long)n;
    }
    pthread_mutex_unlock(&g_log_mtx);
#endif
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
/* Cached descriptors. Reads and writes need *separate* descriptors: on this
 * kernel a read through an O_RDWR /proc/self/mem returns EIO, which silently
 * breaks every rd() and wedges the UI logic that depends on it. */
static int mem_rd_fd = -1;
static int mem_wr_fd = -1;
static pthread_mutex_t g_mem_mtx = PTHREAD_MUTEX_INITIALIZER;

static int rd(uintptr_t addr, void *dst, size_t n)
{
    pthread_mutex_lock(&g_mem_mtx);
    if (mem_rd_fd < 0)
        mem_rd_fd = open("/proc/self/mem", O_RDONLY);
    ssize_t got = mem_rd_fd < 0 ? -1 : pread(mem_rd_fd, dst, n, (off_t)addr);
    pthread_mutex_unlock(&g_mem_mtx);
    return got == (ssize_t)n;
}

static int wr(uintptr_t addr, const void *src, size_t n)
{
    pthread_mutex_lock(&g_mem_mtx);
    if (mem_wr_fd < 0)
        mem_wr_fd = open("/proc/self/mem", O_WRONLY);
    ssize_t put = mem_wr_fd < 0 ? -1 : pwrite(mem_wr_fd, src, n, (off_t)addr);
    pthread_mutex_unlock(&g_mem_mtx);
    return put == (ssize_t)n;
}

static int mapped(const void *p)
{
    /* Ranges are loaded lazily (the heap grows after load), so "unknown" must
     * stay permissive: rd()/wr() fail safely on unmapped memory anyway. The only
     * writer that trusted this blindly was the debug `set` channel, which is now
     * compiled out of release builds. */
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
    if ((insn & 0x7C000000u) == 0x14000000u) /* B / BL            */
        return 1;
    if ((insn & 0x9F000000u) == 0x90000000u) /* ADRP              */
        return 1;
    if ((insn & 0x9F000000u) == 0x10000000u) /* ADR               */
        return 1;
    if ((insn & 0x3B000000u) == 0x18000000u) /* LDR literal       */
        return 1;
    if ((insn & 0x7E000000u) == 0x34000000u) /* CBZ / CBNZ        */
        return 1;
    if ((insn & 0x7E000000u) == 0x36000000u) /* TBZ / TBNZ        */
        return 1;
    /* Indirect branches and returns must not be copied into a trampoline. */
    if ((insn & 0xFFFFFC1Fu) == 0xD61F0000u) /* BR  xN            */
        return 1;
    if ((insn & 0xFFFFFC1Fu) == 0xD63F0000u) /* BLR xN            */
        return 1;
    if ((insn & 0xFFFFFBFFu) == 0xD65F0000u) /* RET               */
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
    /* paciasp/bti are the usual prologue, but many functions start with a
     * plain instruction.  Any non-PC-relative instruction can be copied into
     * the trampoline just as safely. */
    if (pc_relative(first)) {
        mlog("minimod: site %p starts PC-relative (%08x)\n", (void *)site, first);
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
        mlog("minimod: hook %p out of B range\n", (void *)site);
        return 0;
    }
    uint32_t ldr = 0x58000050u; /* ldr x16, [pc, #8] */
    uint32_t br = 0xD61F0200u;  /* br  x16         */
    memcpy(slot, &ldr, 4);
    memcpy(slot + 4, &br, 4);
    memcpy(slot + 8, &hook, 8);
    memcpy(tr, pro, n * 4);
    memcpy(tr + n * 4, &b_body, 4);

    uintptr_t page = site & ~(uintptr_t)0xfff;
    if (mprotect((void *)page, 0x2000, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) {
        mlog("minimod: mprotect(%p) failed: %s\n", (void *)page, strerror(errno));
        return 0;
    }
    int ok = wr(site, &b_slot, 4);
    __builtin___clear_cache((char *)site, (char *)(site + 4));
    mprotect((void *)page, 0x2000, PROT_READ | PROT_EXEC);
    if (!ok) {
        mlog("minimod: patch of %p failed\n", (void *)site);
        return 0;
    }
    nslots++;
    ntramp++;
    __builtin___clear_cache((char *)tr, (char *)tr + n * 4 + 4);
    __builtin___clear_cache((char *)slot, (char *)slot + 16);
    relay_used = 1;
    mlog("minimod: hook %p (prologue %u)\n", (void *)site, n);
    return tr;
}

/* -------------------------------------------------------- 16 Levels state */
static int g_octave;
static volatile int g_levels; /* 16 LEVELS mode, tracked from button 0x28 */
static int g_plus_held, g_minus_held;   /* octave shift applied while 16 LEVELS is active */
static void *g_sixteen; /* the live 16 Levels screen, if seen             */

/* Chop mode replaces the Trim group with the Chop group on the Sample page.
 * The two groups are mutually exclusive children of the screen object, so the
 * live visibility of the Chop group (+0x1ee0) is the mode. Cross-checked
 * against the panel's CHOP LED: LED bright <=> +0x1ee0 visible. */
static int in_chop_mode(void)
{
    void *s = g_screen;
    if (!s || FIELD(s, 0, uintptr_t) != base + 0x6e212a0) /* SampleModeScreen */
        return 0;
    return FN(0x112fd50, int (*)(void *))((char *)s + 0x1ee0) != 0;
}

/* 16 LEVELS has no readable state field we could find; the only way in or out
 * on the Sample page is the 16 LEVELS button (code 0x28), which is a toggle.
 * The mode also ends when the Sample page stops being shown, so the tracked
 * flag is dropped whenever the screen is not visible. */
static int levels_active(void)
{
    void *s = g_screen;
    if (!s || FIELD(s, 0, uintptr_t) != base + 0x6e212a0) /* SampleModeScreen */
        return 0;
    if (!FN(0x112fd50, int (*)(void *))(s)) {
        g_levels = 0;
        return 0;
    }
    return g_levels;
}

static int sixteen_tune_active(void)
{
    return g_octave != 0;
}

static int bump_octave(int d)
{
    mlog("minimod: PLUS/MINUS -> %+d x 8 semitones\n", d);
    int next = g_octave + d;
    if (next < -4)
        next = -4;
    if (next > 4)
        next = 4;
    if (next == g_octave)
        return 0;
    g_octave = next;
    mlog("minimod: pitch shift=%d (%+d semitones)\n", next, next * 8);
    if (next == 0) {
        toast("Pitch reset");           /* back to the original pitch */
    } else {
        char buf[48];
        snprintf(buf, sizeof buf, "Pitch %+d semitones", next * 8);
        toast(buf);
    }
    return 1;
}

/* ------------------------------------------------------------- WAV helpers */
static int wav_info(const char *path, unsigned *frames, unsigned *rate,
                    unsigned *ch, unsigned *dataoff, unsigned *bps, unsigned *format)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    unsigned char h[12];
    if (pread(fd, h, 12, 0) != 12 || memcmp(h, "RIFF", 4) || memcmp(h + 8, "WAVE", 4)) {
        close(fd);
        return 0;
    }
    /* Walk the chunks and take fmt where it actually is: recordings carry a
     * JUNK chunk first, so the fmt fields are not at fixed offsets. */
    unsigned encoding = 0, chn = 0, bits = 0, rt = 0, dof = 0, len = 0;
    off_t off = 12;
    for (int guard = 0; guard < 64; ++guard) {
        unsigned char ck[8];
        if (pread(fd, ck, 8, off) != 8)
            break;
        unsigned sz = (unsigned)ck[4] | ((unsigned)ck[5] << 8) |
                      ((unsigned)ck[6] << 16) | ((unsigned)ck[7] << 24);
        if (!memcmp(ck, "fmt ", 4) && sz >= 16 && sz <= 40) {
            unsigned char f[40] = {0};
            if (pread(fd, f, sz, off + 8) != (ssize_t)sz)
                break;
            encoding = (unsigned)f[0] | ((unsigned)f[1] << 8);
            chn = (unsigned)f[2] | ((unsigned)f[3] << 8);
            rt = (unsigned)f[4] | ((unsigned)f[5] << 8) |
                 ((unsigned)f[6] << 16) | ((unsigned)f[7] << 24);
            bits = (unsigned)f[14] | ((unsigned)f[15] << 8);
            if (encoding == 0xfffe && sz >= 40)   /* EXTENSIBLE: subformat GUID */
                encoding = (unsigned)f[24] | ((unsigned)f[25] << 8);
        } else if (!memcmp(ck, "data", 4)) {
            dof = (unsigned)off + 8;
            len = sz;
            if (encoding)
                break;
        }
        off += 8 + (off_t)sz + (off_t)(sz & 1);
    }
    close(fd);
    if (!dof || !len || !chn || !rt || !encoding)
        return 0;
    if (encoding != 1 && !(encoding == 3 && bits == 32))
        return 0;
    if (bits != 16 && bits != 24 && bits != 32)
        return 0;
    *frames = len / ((bits / 8u) * chn);
    *rate = rt;
    *ch = chn;
    *dataoff = dof;
    *bps = bits / 8u;
    if (format) *format = encoding;
    return 1;
}

static int pcm_at(const unsigned char *p, unsigned bps, unsigned format)
{
    if (format == 3) {
        float value;
        memcpy(&value, p, sizeof value);
        if (!isfinite(value)) return 0;
        if (value >= 1.0f) return 32767;
        if (value <= -1.0f) return -32768;
        return (int)(value * 32768.0f);
    }
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

/* SHIFT+B3 cycles 3 % -> 6 % -> 9 % -> untrimmed while SHIFT stays held;
 * releasing SHIFT resets the cycle. */
static int g_trim_step;
static double g_trim_ratio;
static int g_b3_down;      /* edge detection: the button auto-repeats while held */

static int wav_edges(const char *path, unsigned frames, unsigned dataoff,
                     unsigned ch, unsigned bps, unsigned format, unsigned *first, unsigned *last)
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
     * nothing. g_trim_ratio is set per SHIFT+B3 press (3 %, then 6 %);
     * MINIMOD_TRIM_DB overrides the relative part for the default case. */
    unsigned char buf[16384];
    short thr = 328; /* -40 dBFS fallback */
    {
        double db = -30.45757490560675; /* 3% of peak: three times the old threshold */
        double ratio;
        if (g_trim_ratio > 0.0) {
            ratio = g_trim_ratio;
        } else {
            const char *e = getenv("MINIMOD_TRIM_DB");
            if (e && *e)
                db = atof(e);
            if (db < -90.0) db = -90.0;
            if (db > -6.0) db = -6.0;
            ratio = pow(10.0, db / 20.0);
        }
        int peak = 0;
        unsigned g = 0;
        while (g < frames) {
            ssize_t n = pread(fd, buf, sizeof buf, (off_t)dataoff + (off_t)g * bps * ch);
            if (n <= 0)
                break;
            unsigned got = (unsigned)(n / (bps * ch));
            for (unsigned i = 0; i < got; ++i)
                for (unsigned c = 0; c < ch; ++c) {
                    int v = pcm_at(buf + (i * ch + c) * bps, bps, format);
                    if (v < 0) v = -v;
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
            mlog("minimod: trim threshold ratio %.4f peak %d -> %d\n", ratio,
                 (int)peak, (int)thr);
            if (getenv("MINIMOD_ENV")) {
                /* level envelope in 32 windows, same scale as the threshold */
                unsigned step = frames / 32 ? frames / 32 : 1;
                for (unsigned w = 0; w < 32; ++w) {
                    unsigned a = w * step, b = a + step;
                    if (b > frames)
                        b = frames;
                    int wp = 0;
                    for (unsigned f = a; f < b; f += 1) {
                        unsigned char sb[8];
                        if (!rd(0, sb, 0))
                            break;
                        /* read one frame */
                        if (pread(fd, sb, (size_t)(bps * ch), (off_t)dataoff + (off_t)f * bps * ch) <= 0)
                            break;
                        for (unsigned c = 0; c < ch; ++c) {
                            int v = pcm_at(sb + c * bps, bps, format);
                            if (v < 0) v = -v;
                            if (v > wp) wp = v;
                        }
                    }
                    mlog("minimod: env %2u/%02u peak %5d\n", w, 32, (int)wp);
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
                int v = pcm_at(buf + (i * ch + c) * bps, bps, format);
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
                int v = pcm_at(buf + (i * ch + c) * bps, bps, format);
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
    mlog("minimod: trimparams found=%d\n", g_ntp);
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
        if (!wav_info(tmp, &fr, &rt, &ch, &doff, &bps, NULL) || fr != want)
            continue;
        snprintf(out, n, "%s", tmp);
        return 1;
    }
    return 0;
}

/* A freshly recorded (unsaved) sample has no path anywhere near its object, but
 * the app has already written the audio into the temp project. Find the file by
 * its frame count - the recording's own length is a strong identifier. */
static int find_wav_by_frames(unsigned want, char *out, size_t n)
{
    DIR *root = opendir("/data/TempProjects");
    if (!root)
        return 0;
    struct dirent *de;
    char best[512] = {0};
    time_t best_t = 0;
    while ((de = readdir(root)) != 0) {
        if (de->d_name[0] == '.')
            continue;
        char dirp[512];
        snprintf(dirp, sizeof dirp, "/data/TempProjects/%s", de->d_name);
        struct stat ds;
        if (stat(dirp, &ds) != 0 || !S_ISDIR(ds.st_mode))
            continue;
        DIR *sub = opendir(dirp);
        if (!sub)
            continue;
        struct dirent *fe;
        while ((fe = readdir(sub)) != 0) {
            size_t l = strlen(fe->d_name);
            if (l < 5 || strcasecmp(fe->d_name + l - 4, ".wav") != 0)
                continue;
            char full[512];
            snprintf(full, sizeof full, "%s/%s", dirp, fe->d_name);
            unsigned fr = 0, rate = 0, ch = 0, off = 0, bytes = 0, fmt = 0;
            if (!wav_info(full, &fr, &rate, &ch, &off, &bytes, &fmt) || fr != want)
                continue;
            struct stat fs;
            if (stat(full, &fs) != 0)
                continue;
            if (fs.st_mtime >= best_t) {
                best_t = fs.st_mtime;
                snprintf(best, sizeof best, "%s", full);
            }
        }
        closedir(sub);
    }
    closedir(root);
    if (!best[0])
        return 0;
    snprintf(out, n, "%s", best);
    return 1;
}

/* The sample object carries its file: sample+0x30 is the holder, whose +0x120
 * is a std::string with the path. Read it directly before falling back to the
 * generic scan or the frame-count lookup. */
static int path_from_object(uintptr_t sample, unsigned want, char *out, size_t n)
{
    uintptr_t holder = 0, sptr = 0;
    char buf[512];
    if (!rd(sample + 0x30, &holder, 8) || !holder || !mapped((void *)holder))
        return 0;
    if (!rd(holder + 0x120, &sptr, 8) || !sptr || !mapped((void *)sptr))
        return 0;
    if (!rd(sptr, buf, sizeof buf - 1))
        return 0;
    buf[sizeof buf - 1] = 0;
    return path_from_buffer(buf, want, out, n);
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
            mlog("minimod: path from sample+0x%lx -> %s\n", (unsigned long)o, out);
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
                mlog("minimod: path from sample+0x%lx/+0x%lx -> %s\n",
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
            int ok = wav_info(path, &fr, &rt, &ch, &doff, &bps, NULL);
            static int logged;
            if (logged < 10) {
                logged++;
                mlog("minimod: wav '%s' ok=%d frames=%u ch=%u\n", f->d_name, ok,
                     fr, ch);
            }
            if (!ok || fr != want)
                continue;
            if (found) {
                closedir(d2);
                closedir(d);
                mlog("minimod: autotrim: %u frames matches more than one wav\n", want);
                return 0;
            }
            snprintf(out, n, "%s", path);
            found = 1;
        }
        closedir(d2);
    }
    closedir(d);
    if (found)
        mlog("minimod: autotrim: wav by frames -> %s\n", out);
    return found;
}

/* Decode a whole WAV into interleaved float32.  This is intentionally stricter
 * than wav_info(): HPSS must read exactly the sample the app has loaded, so the
 * frame count, channel count and rate all have to match the native metadata.
 * Supports PCM16/24/32 and IEEE float32/64, including WAVE_FORMAT_EXTENSIBLE. */
static int wav_read_f32(const char *path, unsigned want_frames, unsigned want_ch,
                        unsigned want_rate, float **out, unsigned *got_ch,
                        unsigned *got_rate)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    unsigned char h[12];
    if (pread(fd, h, 12, 0) != 12 || memcmp(h, "RIFF", 4) ||
        memcmp(h + 8, "WAVE", 4)) {
        close(fd);
        return 0;
    }
    unsigned encoding = 0, ch = 0, bits = 0, rate = 0, dof = 0, len = 0;
    off_t off = 12;
    for (int guard = 0; guard < 64; ++guard) {
        unsigned char ck[8];
        if (pread(fd, ck, 8, off) != 8)
            break;
        unsigned sz = (unsigned)ck[4] | ((unsigned)ck[5] << 8) |
                      ((unsigned)ck[6] << 16) | ((unsigned)ck[7] << 24);
        if (!memcmp(ck, "fmt ", 4) && sz >= 16 && sz <= 40) {
            unsigned char f[40] = {0};
            if (sz > sizeof f || pread(fd, f, sz, off + 8) != (ssize_t)sz)
                break;
            encoding = (unsigned)f[0] | ((unsigned)f[1] << 8);
            ch = (unsigned)f[2] | ((unsigned)f[3] << 8);
            rate = (unsigned)f[4] | ((unsigned)f[5] << 8) |
                   ((unsigned)f[6] << 16) | ((unsigned)f[7] << 24);
            bits = (unsigned)f[14] | ((unsigned)f[15] << 8);
            if (encoding == 0xfffe && sz >= 40)
                encoding = (unsigned)f[24] | ((unsigned)f[25] << 8);
        } else if (!memcmp(ck, "data", 4)) {
            dof = (unsigned)off + 8;
            len = sz;
            if (encoding)
                break;
        }
        off += 8 + (off_t)sz + (off_t)(sz & 1);
    }
    if (!dof || !len || !ch || ch > 2 || !rate || !bits)
        goto bad;
    if (!((encoding == 1 && (bits == 16 || bits == 24 || bits == 32)) ||
          (encoding == 3 && (bits == 32 || bits == 64))))
        goto bad;
    unsigned bps = bits / 8;
    unsigned frames = len / (bps * ch);
    if (frames != want_frames || ch != want_ch || rate != want_rate)
        goto bad;

    size_t cells = (size_t)frames * ch;
    if (cells > SIZE_MAX / sizeof(float))
        goto bad;
    float *buf = malloc(cells * sizeof(float));
    if (!buf)
        goto bad;
    unsigned char tmp[8192];
    size_t at = 0;
    while (at < cells) {
        size_t want = cells - at;
        if (want > sizeof tmp / bps)
            want = sizeof tmp / bps;
        ssize_t n = pread(fd, tmp, want * bps, (off_t)dof + (off_t)at * bps);
        if (n <= 0)
            break;
        size_t got = (size_t)n / bps;
        for (size_t i = 0; i < got; ++i) {
            const unsigned char *p = tmp + i * bps;
            if (encoding == 3 && bits == 32) {
                float v;
                memcpy(&v, p, 4);
                buf[at + i] = v;
            } else if (encoding == 3 && bits == 64) {
                double v;
                memcpy(&v, p, 8);
                buf[at + i] = (float)v;
            } else if (bits == 16) {
                int16_t v;
                memcpy(&v, p, 2);
                buf[at + i] = (float)v / 32768.0f;
            } else if (bits == 24) {
                int v = (int)p[0] | ((int)p[1] << 8) |
                        ((int)(signed char)p[2] << 16);
                buf[at + i] = (float)v / 8388608.0f;
            } else {
                int32_t v;
                memcpy(&v, p, 4);
                buf[at + i] = (float)((double)v / 2147483648.0);
            }
        }
        at += got;
        if (got != want)
            break;
    }
    close(fd);
    if (at != cells) {
        free(buf);
        return 0;
    }
    *out = buf;
    if (got_ch)
        *got_ch = ch;
    if (got_rate)
        *got_rate = rate;
    return 1;
bad:
    close(fd);
    return 0;
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
        mlog("minimod: no Sample screen found\n");
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
        mlog("minimod: sample obj=%p info=%p frames=%u path='%s'\n", (void *)child,
             (void *)info, fr, p2[0] ? p2 : "?");
        return p2[0] ? 1 : 0;
    }
    mlog("minimod: displayed sample struct not found (screen=%p)\n", screen);
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
static void (*orig_midi_note_event)(void *, void *, void *, uintptr_t);
static volatile uint32_t g_midi_chromatic_count;
static volatile int g_midi_chromatic_note = -1;
static volatile int g_midi_chromatic_delta;
static void (*orig_bass_audio_writer)(void *, void *, size_t, int);
static void (*orig_cs)(void *, int, void *);
static void (*orig_trim_write)(void *, long, long);
static void (*orig_frames)(void *);
static void (*orig_trim)(void *, float);
static void (*orig_resized)(void *, long, long, long);
static void (*orig_reg)(void *, void *);
static void *(*orig_prog_inst)(void *, int);
/* The Sample page (0x2468 bytes) captured from SamplePage_Ctor; it owns the
 * Start/End widgets and is what must be repainted after a property change. */
static void *g_page;
typedef void (*pagector_fn)(void *, void *, void *, void *, void *, void *,
                            void *, void *, void *, void *, void *, void *);
static pagector_fn orig_pagector;

static void pagector_probe(void *a0, void *a1, void *a2, void *a3, void *a4, void *a5,
                           void *a6, void *a7, void *a8, void *a9, void *a10, void *a11)
{
    uintptr_t vt = 0;
    if (a0 && rd((uintptr_t)a0, &vt, 8) && vt >= base && vt <= base + 0x12000000ULL)
        g_page = a0;
    if (orig_pagector)
        orig_pagector(a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11);
}

/* The app's own "apply a control value" entry - exactly what a knob turn calls
 * (FUN_01a68020(0, surface, controlId, value, mode)). Capturing the surface lets
 * commit_trim drive the Start/End controls the same way the UI does. */
static void *g_surface;
static void *g_control_context;
static void (*orig_setctl)(void *, void *, int, int, float);

static void setctl_probe(void *a0, void *a1, int a2, int a3, float v)
{
    uintptr_t vt = 0;
    if (a1 && rd((uintptr_t)a1, &vt, 8) && vt >= base && vt <= base + 0x12000000ULL) {
        g_surface = a1;
        g_control_context = a0;
    }
    if (orig_setctl)
        orig_setctl(a0, a1, a2, a3, v);
}

/* The page object, captured from the control handler FUN_02904e90(page, id, ...)
 * - refreshed on every knob turn, so it is always a live pointer. */
static void *g_livepage;
static void (*orig_ctlhandler)(void *, uint64_t, uint64_t);

static void ctlhandler_probe(void *a0, uint64_t a1, uint64_t a2)
{
    uintptr_t vt = 0;
    if (a0 && rd((uintptr_t)a0, &vt, 8) && vt >= base && vt <= base + 0x12000000ULL) {
        uintptr_t arr = 0;
        /* arr is the pad-block array - a heap pointer, not an image one. */
        if (rd((uintptr_t)a0 + 0x22e8, &arr, 8) &&
            arr > 0xffff00000000ULL && arr < 0xffffff000000ULL)
            g_livepage = a0;
    }
    if (orig_ctlhandler)
        orig_ctlhandler(a0, a1, a2);
}

/* FUN_02912f10(padBlock) returns the object the page reads its trim from
 * (Start at +0x78, End at +0xd0). Capturing it in-process avoids stale pointers. */
static void *g_trimobj;
static void *(*orig_gettrim)(void *);

static void *gettrim_probe(void *blk)
{
    void *r = orig_gettrim ? orig_gettrim(blk) : 0;
    if (r && (uintptr_t)r > 0xffff00000000ULL && (uintptr_t)r < 0xffffff000000ULL) {
        static int n;
        if (g_trimobj != r || n < 3) {
            int64_t f0 = -1, f1 = -1;
            rd((uintptr_t)r + 0x78, &f0, sizeof f0);
            rd((uintptr_t)r + 0xd0, &f1, sizeof f1);
            mlog("minimod: trimobj=%p (blk=%p) start=%lld end=%lld\n",
                 r, blk, (long long)f0, (long long)f1);
            n++;
        }
        g_trimobj = r;
    }
    return r;
}

/* PadState_CopyIntoPage(page, padState): the pad is the source of the page's
 * Start/End widgets (pad+0x1588 -> widget value, pad+0x15e0 -> End). */
static void *g_padstate;
static void (*orig_copypad)(void *, void *);

static void copypad_probe(void *page, void *pad)
{
    uintptr_t vt = 0;
    if (pad && (uintptr_t)pad > 0xffff00000000ULL && (uintptr_t)pad < 0xffffff000000ULL) {
        float f0 = -1, f1 = -1;
        rd((uintptr_t)pad + 0x1588, &f0, 4);
        rd((uintptr_t)pad + 0x15e0, &f1, 4);
        mlog("minimod: pad=%p start=%.4f end=%.4f\n", pad, (double)f0, (double)f1);
        g_padstate = pad;
    }
    if (orig_copypad)
        orig_copypad(page, pad);
}

static void (*orig_bcast)(void *, float);
static void (*orig_cc)(void *, unsigned, float, float);
static void (*orig_sync)(void *, void *, void *, void *);

static void sync_probe(void *a0, void *a1, void *a2, void *a3)
{
    static int n;
    if (n < 12) {
        n++;
        float f0 = -1, f1 = -1, g0 = -1, g1 = -1;
        if (a0) {
            rd((uintptr_t)a0 + 0x1ac0, &f0, 4);
            rd((uintptr_t)a0 + 0x1b18, &f1, 4);
        }
        if (a1) {
            rd((uintptr_t)a1 + 0x1ac0, &g0, 4);
            rd((uintptr_t)a1 + 0x1b18, &g1, 4);
        }
        mlog("minimod: sync a0=%p(a0k1=%.4f a0k2=%.4f) a1=%p(a1k1=%.4f a1k2=%.4f)\n",
             a0, (double)f0, (double)f1, a1, (double)g0, (double)g1);
    }
    if (orig_sync)
        orig_sync(a0, a1, a2, a3);
}
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
            mlog("minimod: locator %p pos=%.4f\n", (void *)s, (double)pos);
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
        mlog("minimod: 16 levels ctor %p\n", g_sixteen);
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
        mlog("minimod: pitch offset %d -> %d\n", a3, off);
    if (orig_pitch_off)
        orig_pitch_off(a0, a1, a2, off);
}

static void heap_snapshot(const char *tag);
static void heap_diff(const char *tag);
static uintptr_t g_last_press_obj;
static void find_float_pair(float v);
/* "findv <float>" - every address in the small writable ranges holding exactly
 * this float. Run with two knob values and intersect to find the stored state. */
static void find_value(float v)
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
        if (g_rng[r].hi - g_rng[r].lo > 0x8000000)
            continue; /* skip only the very large buffers */
        for (uintptr_t a = g_rng[r].lo; a + 4 < g_rng[r].hi; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)3;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 4 <= n && shown < 60; o += 4) {
                if (memcmp(buf + o, pat, 4) == 0) {
                    mlog("minimod: fv %#llx %.6f\n",
                         (unsigned long long)(a + o), (double)v);
                    shown++;
                }
            }
        }
    }
    mlog("minimod: fv done %d\n", shown);
    free(buf);
}

static void byte_snap(void);
static void byte_diff(void);
static void find_value(float v);
static void (*orig_screen_paint)(void *, void *);
static volatile int g_pending_ctl;
static float g_pending_v0, g_pending_v1;
static int g_pending_r0, g_pending_r1;
static void (*orig_setctl2)(void *, void *, int, int, float);
static void *g_surface2;

static void *g_pend_prop0, *g_pend_prop1;
static float g_pend_pv0, g_pend_pv1;
static volatile int g_pending_prop;

/* Write the property values immediately before the app paints, so the paint
 * reads the trim we computed. */
static void apply_pending_props(void)
{
    float v;
    if (!g_pending_prop)
        return;
    g_pending_prop = 0;
    if (g_pend_prop0) {
        v = g_pend_pv0;
        wr((uintptr_t)g_pend_prop0 + 0x50, &v, 4);
    }
    if (g_pend_prop1) {
        v = g_pend_pv1;
        wr((uintptr_t)g_pend_prop1 + 0x50, &v, 4);
    }
    mlog("minimod: props written in paint %.4f/%.4f\n",
         (double)g_pend_pv0, (double)g_pend_pv1);
}

static void apply_pending_ctl(void)
{
    if (!g_pending_ctl)
        return;
    g_pending_ctl = 0;
    if (!g_surface2 || !orig_setctl2)
        return;
    uintptr_t svt = 0;
    if (!rd((uintptr_t)g_surface2, &svt, 8) || svt < base || svt > base + 0x12000000ULL)
        return;
    if (!g_control_context)
        return;
    orig_setctl2(g_control_context, g_surface2, 0, 0x466, g_pending_v1);
    orig_setctl2(g_control_context, g_surface2, 0, 0x462, g_pending_v0);
    mlog("minimod: paint-time setctl %d/%d\n", g_pending_r0, g_pending_r1);
}
static void *g_paint_comp;
static void *g_paint_gfx;
static void who_points_at(uintptr_t target);
static uintptr_t g_reg_base;
static unsigned char g_reg_buf[0x40000];
static int g_reg_have;
static volatile int g_poke_which = -1;
static void do_poke(void);
static volatile int g_sdump;
static void dump_sliders(void *p);
static volatile float g_poke_value;
static float g_mark_v0 = -1.0f, g_mark_v1 = -1.0f;
static volatile int g_pending_cc;
static volatile int g_in_inject;
static int g_cc_step;
static uintptr_t g_cc_obj;
static unsigned g_cc_r0, g_cc_r1;
static void commit_trim(void);

/* ---------------------------------------------------------- HPSS bridge */
/* Stock Chop Extract, used as the sample-creation/assignment mechanism.  The
 * handler finds the first empty pad, copies the source pad settings and submits
 * an ExtractAndAssign command.  We only replace the audio that command writes
 * and the name it gives the result; everything else stays native. */
#define A_HPSS_EXTRACT_HANDLER 0x16580d0
#define A_HPSS_EXTRACT_CTOR 0x143ed40
#define A_HPSS_EXTRACT_EXEC 0x143f0b0
#define A_HPSS_WRITE_CB 0x1d8aff0
#define A_HPSS_FREE_PAD 0x286dd00
#define A_HPSS_PAD_BUSY 0x28047b0
#define HPSS_RANGE_START 0x78
#define HPSS_RANGE_END 0xd0

static void (*orig_hpss_ctor)(void *, void *, int, void *, void *, void *, void *,
                              void *, void *);
static void (*orig_hpss_exec)(void *);
static void (*orig_hpss_writer)(void *, void *);

/* One armed output.  The constructor hook maps the command it sees to this
 * buffer, and the writer callback consumes the buffer sequentially. */
static volatile int g_hpss_arm;
static uintptr_t g_hpss_arm_source;
static const float *g_hpss_arm_buf;
static size_t g_hpss_arm_frames;
static size_t g_hpss_arm_source_frames;
static unsigned g_hpss_arm_start;
static unsigned g_hpss_arm_end;
static unsigned g_hpss_arm_ch;
static int g_hpss_arm_name;
static volatile int g_hpss_sub_on;
static const float *g_hpss_sub_buf;
static volatile size_t g_hpss_sub_pos;
static size_t g_hpss_sub_frames;
static size_t g_hpss_sub_source_frames;
static unsigned g_hpss_sub_ch;
static volatile int g_hpss_write_ok;
static void *g_hpss_cmd[4];
static int g_hpss_cmd_n;

static uint64_t g_hpss_name[6];
static int g_hpss_names_ready;
static int g_spectral_active;
static volatile int g_hpss_menu;
static volatile int g_hpss_busy;
static volatile int g_hpss_sel;
static volatile int g_bass_edit;
static float g_bass_knob[3] = {0.52f, 0.18f, 0.56f};
static volatile unsigned g_bass_preview_notes;
static volatile unsigned g_bass_preview_blocks;
static volatile uint32_t g_bass_preview_peak_bits;
static volatile int g_bass_preview_active;
static volatile int g_bass_preview_available;
static uint32_t bass_float_bits(float value)
{
    uint32_t bits;
    memcpy(&bits, &value, sizeof bits);
    return bits;
}
static void bass_preview_set_active(int active)
{
    if (active && !g_bass_preview_available)
        active = 0;
    if (active) {
        __atomic_store_n(&g_bass_preview_notes, 0, __ATOMIC_RELAXED);
        __atomic_store_n(&g_bass_preview_blocks, 0, __ATOMIC_RELAXED);
        __atomic_store_n(&g_bass_preview_peak_bits, 0, __ATOMIC_RELAXED);
    }
    __atomic_store_n(&g_bass_preview_active, active != 0, __ATOMIC_RELEASE);
    bass_preview_enable(active);
}
/* Native DATA input: the decoded control surface emits signed deltas through
 * Ghidra FUN_0150dbc0(surface + 0x2da8, delta). Unlike the float broadcaster,
 * this is an event: repeated identical deltas must each move the selection.
 * Learn the surface from the same native event that opens SAMPLE OPTIONS. */
static uintptr_t g_data_signal;
static void (*orig_data_signal)(void *, int);
static void hpss_repaint(void);

static void data_signal(void *self, int delta)
{
    if ((uintptr_t)self == __atomic_load_n(&g_data_signal, __ATOMIC_ACQUIRE) &&
        (g_hpss_menu || __atomic_load_n(&g_spectral_active, __ATOMIC_ACQUIRE))) {
        if (g_hpss_menu && !g_hpss_busy && delta) {
            if (!g_bass_edit) {
                g_hpss_sel = (g_hpss_sel + (delta > 0 ? 1 : 6)) % 7;
                hpss_repaint();
                mlog("menu DATA delta=%d sel=%d\n", delta, g_hpss_sel);
            }
        }
        return; /* consume only this surface's DATA event, including busy */
    }
    if (orig_data_signal)
        orig_data_signal(self, delta);
}

static void hpss_make_names(void)
{
    if (g_hpss_names_ready)
        return;
    FN(0xf535c0, void (*)(void *, const char *))(&g_hpss_name[0], "HPSS Harmonic");
    FN(0xf535c0, void (*)(void *, const char *))(&g_hpss_name[1], "HPSS Percussive");
    FN(0xf535c0, void (*)(void *, const char *))(&g_hpss_name[2], "Reverb");
    FN(0xf535c0, void (*)(void *, const char *))(&g_hpss_name[3], "Vinyl");
    FN(0xf535c0, void (*)(void *, const char *))(&g_hpss_name[4], "Lo-Fi");
    FN(0xf535c0, void (*)(void *, const char *))(&g_hpss_name[5], "Bass C2");
    g_hpss_names_ready = 1;
}

static void hpss_ctor_hook(void *cmd, void *name, int dest, void *settings,
                           void *padstate, void *project, void *s1, void *s2,
                           void *s3)
{
    if (g_hpss_arm && (uintptr_t)padstate == g_hpss_arm_source) {
        hpss_make_names();
        int ni = g_hpss_arm_name;
        if (ni < 0 || ni >= 6)
            ni = 0;
        name = (void *)&g_hpss_name[ni];
        if (g_hpss_cmd_n < 4)
            g_hpss_cmd[g_hpss_cmd_n++] = cmd;
        mlog("HPSS ctor: cmd=%p dest=%d settings=%p name_index=%d\n", cmd, dest,
             settings, ni);
    }
    if (orig_hpss_ctor)
        orig_hpss_ctor(cmd, name, dest, settings, padstate, project, s1, s2, s3);
}

static void hpss_exec_hook(void *cmd)
{
    int armed = 0;
    for (int i = 0; i < g_hpss_cmd_n; ++i)
        if (g_hpss_cmd[i] == cmd)
            armed = 1;
    uintptr_t settings = 0;
    uint64_t old_start = 0, old_end = 0;
    int patched = 0;
    if (armed && g_hpss_arm) {
        g_hpss_write_ok = 0;
        g_hpss_sub_on = 0;
        if (g_hpss_arm_buf) {
            g_hpss_sub_buf = g_hpss_arm_buf;
            g_hpss_sub_frames = g_hpss_arm_frames;
            g_hpss_sub_source_frames = g_hpss_arm_source_frames;
            g_hpss_sub_ch = g_hpss_arm_ch;
            g_hpss_sub_pos = 0;
            g_hpss_sub_on = 1;
        }
        /* The constructor stores the settings object by pointer; the execute
         * method is what copies its range into the slice command.  Patch it
         * here, for exactly the duration of that copy. */
        if (rd((uintptr_t)cmd + 0x60, &settings, 8) && settings &&
            rd(settings + HPSS_RANGE_START, &old_start, 8) &&
            rd(settings + HPSS_RANGE_END, &old_end, 8)) {
            uint64_t start = g_hpss_arm_start, end = g_hpss_arm_end;
            patched = wr(settings + HPSS_RANGE_START, &start, 8) &&
                      wr(settings + HPSS_RANGE_END, &end, 8);
            mlog("HPSS exec: cmd=%p settings range %llu..%llu -> %u..%u\n", cmd,
                 (unsigned long long)old_start, (unsigned long long)old_end,
                 g_hpss_arm_start, g_hpss_arm_end);
        }
        mlog("HPSS exec: cmd=%p armed %u..%u (%zu frames) x %u ch buf=%p\n", cmd,
             g_hpss_arm_start, g_hpss_arm_end, g_hpss_arm_frames,
             g_hpss_arm_ch, (void *)g_hpss_arm_buf);
        if (!g_hpss_arm_buf)
            g_hpss_write_ok = 1; /* native Extract copies the range itself */
    }
    if (orig_hpss_exec)
        orig_hpss_exec(cmd);
    if (patched) {
        wr(settings + HPSS_RANGE_START, &old_start, 8);
        wr(settings + HPSS_RANGE_END, &old_end, 8);
    }
    if (armed && g_hpss_sub_on) {
        /* The native writer may run on an executor thread.  Wait for it so the
         * buffer stays alive and the next extraction starts from a clean state. */
        int spins = 0;
        while (g_hpss_sub_pos < g_hpss_sub_frames && spins++ < 3000)
            usleep(1000);
        g_hpss_sub_on = 0;
        g_hpss_write_ok = (g_hpss_sub_pos == g_hpss_sub_frames);
        mlog("HPSS exec: cmd=%p writer consumed %zu/%zu frames%s\n", cmd,
             g_hpss_sub_pos, g_hpss_sub_frames,
             g_hpss_write_ok ? "" : " (INCOMPLETE)");
    }
}

static void hpss_writer_hook(void *closure, void *block)
{
    if (!g_hpss_sub_on || !block) {
        if (orig_hpss_writer)
            orig_hpss_writer(closure, block);
        return;
    }
    unsigned frames = 0;
    float *ptr = NULL;
    memcpy(&frames, block, sizeof frames);
    memcpy(&ptr, (char *)block + 8, sizeof ptr);
    size_t pos = g_hpss_sub_pos;
    if (g_hpss_sub_buf && frames && pos + frames <= g_hpss_sub_source_frames) {
        float *replacement =
            (float *)(g_hpss_sub_buf + pos * (size_t)g_hpss_sub_ch);
        unsigned write_frames = frames;
        int append_tail = pos + frames == g_hpss_sub_source_frames &&
                          g_hpss_sub_frames > g_hpss_sub_source_frames;
        if (append_tail) {
            size_t remaining = g_hpss_sub_frames - pos;
            if (remaining > UINT32_MAX) {
                if (orig_hpss_writer)
                    orig_hpss_writer(closure, block);
                return;
            }
            write_frames = (unsigned)remaining;
            memcpy(block, &write_frames, sizeof write_frames);
        }
        memcpy((char *)block + 8, &replacement, sizeof replacement);
        if (orig_hpss_writer)
            orig_hpss_writer(closure, block);
        if (append_tail)
            memcpy(block, &frames, sizeof frames);
        memcpy((char *)block + 8, &ptr, sizeof ptr);
        g_hpss_sub_pos = append_tail ? g_hpss_sub_frames : pos + frames;
        mlog("HPSS writer: %u frames at %zu%s\n", write_frames, pos,
             append_tail ? " (tail appended)" : "");
    } else {
        mlog("HPSS writer: refusing block frames=%u pos=%zu total=%zu\n", frames,
             pos, g_hpss_sub_frames);
        if (orig_hpss_writer)
            orig_hpss_writer(closure, block);
    }
}

static int hpss_read_sample(void *s, void **screen, uintptr_t *state,
                            uintptr_t *sample, unsigned *frames, unsigned *ch,
                            unsigned *rate);

static int hpss_sample_now(void **screen, uintptr_t *state, uintptr_t *sample,
                           unsigned *frames, unsigned *ch, unsigned *rate)
{
    return hpss_read_sample(g_screen, screen, state, sample, frames, ch, rate);
}

static int hpss_read_sample(void *s, void **screen, uintptr_t *state,
                            uintptr_t *sample, unsigned *frames, unsigned *ch,
                            unsigned *rate)
{
    if (!s || FIELD(s, 0, uintptr_t) != base + A_SCREEN_VT)
        return 0;
    if (!FN(0x112fd50, int (*)(void *))(s))
        return 0;
    uintptr_t binding = 0, st = 0, layers = 0, smp = 0, meta = 0;
    if (!rd((uintptr_t)s + 0x178, &binding, 8) || !binding ||
        !rd(binding + 0x60, &st, 8) || !st ||
        !rd(st + 0x22e8, &layers, 8) || !layers ||
        !rd(layers + 0x4a8, &smp, 8) || !smp ||
        !rd(smp + 0x130, &meta, 8) || !meta)
        return 0;
    unsigned short c = 0;
    unsigned fr = 0, rt = 0;
    if (!rd(meta, &c, 2) || !rd(meta + 4, &fr, 4) || !rd(meta + 8, &rt, 4))
        return 0;
    if ((c != 1 && c != 2) || !fr || rt < 8000 || rt > 192000)
        return 0;
    if (screen)
        *screen = s;
    if (state)
        *state = st;
    if (sample)
        *sample = smp;
    if (frames)
        *frames = fr;
    if (ch)
        *ch = c;
    if (rate)
        *rate = rt;
    return 1;
}

/* Metadata-only reader used to match a path before decoding.  It accepts the
 * same formats as wav_read_f32 (including 64-bit float). */
static int hpss_wav_info(const char *path, unsigned *frames, unsigned *ch,
                         unsigned *rate)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    unsigned char h[12];
    if (pread(fd, h, 12, 0) != 12 || memcmp(h, "RIFF", 4) ||
        memcmp(h + 8, "WAVE", 4)) {
        close(fd);
        return 0;
    }
    unsigned encoding = 0, channels = 0, bits = 0, rt = 0, dof = 0, len = 0;
    off_t off = 12;
    for (int guard = 0; guard < 64; ++guard) {
        unsigned char ck[8];
        if (pread(fd, ck, 8, off) != 8)
            break;
        unsigned sz = (unsigned)ck[4] | ((unsigned)ck[5] << 8) |
                      ((unsigned)ck[6] << 16) | ((unsigned)ck[7] << 24);
        if (!memcmp(ck, "fmt ", 4) && sz >= 16 && sz <= 40) {
            unsigned char f[40] = {0};
            if (sz > sizeof f || pread(fd, f, sz, off + 8) != (ssize_t)sz)
                break;
            encoding = (unsigned)f[0] | ((unsigned)f[1] << 8);
            channels = (unsigned)f[2] | ((unsigned)f[3] << 8);
            rt = (unsigned)f[4] | ((unsigned)f[5] << 8) |
                 ((unsigned)f[6] << 16) | ((unsigned)f[7] << 24);
            bits = (unsigned)f[14] | ((unsigned)f[15] << 8);
            if (encoding == 0xfffe && sz >= 40)
                encoding = (unsigned)f[24] | ((unsigned)f[25] << 8);
        } else if (!memcmp(ck, "data", 4)) {
            dof = (unsigned)off + 8;
            len = sz;
            if (encoding)
                break;
        }
        off += 8 + (off_t)sz + (off_t)(sz & 1);
    }
    close(fd);
    if (!dof || !len || !channels || !rt || !bits)
        return 0;
    if (!((encoding == 1 && (bits == 16 || bits == 24 || bits == 32)) ||
          (encoding == 3 && (bits == 32 || bits == 64))))
        return 0;
    unsigned bps = bits / 8;
    *frames = len / (bps * channels);
    *ch = channels;
    *rate = rt;
    return 1;
}

static int hpss_wav_matches(const char *path, unsigned frames, unsigned ch,
                            unsigned rate)
{
    unsigned fr = 0, c = 0, rt = 0;
    return hpss_wav_info(path, &fr, &c, &rt) && fr == frames && c == ch &&
           rt == rate;
}

/* A fallback for a freshly recorded sample that has no path in its object yet:
 * accept only a unique file with the same frame count, channel count and rate.
 * Frame count alone is not a safe identity. */
static int hpss_wav_unique(unsigned want_frames, unsigned want_ch,
                           unsigned want_rate, char *path, size_t pathsz)
{
    const char *root = "/data/TempProjects";
    DIR *d = opendir(root);
    if (!d)
        return 0;
    struct dirent *de;
    char found[512] = {0};
    int matches = 0;
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
            char full[512];
            snprintf(full, sizeof full, "%s/%s", sub, f->d_name);
            unsigned fr = 0, rt = 0, ch = 0;
            if (!hpss_wav_info(full, &fr, &ch, &rt) || fr != want_frames ||
                rt != want_rate || ch != want_ch)
                continue;
            matches++;
            if (matches == 1)
                snprintf(found, sizeof found, "%s", full);
        }
        closedir(d2);
    }
    closedir(d);
    if (matches != 1)
        return 0;
    snprintf(path, pathsz, "%s", found);
    return 1;
}

static int hpss_path_for(uintptr_t sample, unsigned frames, unsigned ch,
                         unsigned rate, char *path, size_t pathsz)
{
    char candidate[512];
    if (path_from_object(sample, frames, candidate, sizeof candidate) &&
        hpss_wav_matches(candidate, frames, ch, rate)) {
        snprintf(path, pathsz, "%s", candidate);
        return 1;
    }
    if (wav_path_of(sample, frames, candidate, sizeof candidate) &&
        hpss_wav_matches(candidate, frames, ch, rate)) {
        snprintf(path, pathsz, "%s", candidate);
        return 1;
    }
    if (hpss_wav_unique(frames, ch, rate, path, pathsz))
        return 1;
    return 0;
}

static int hpss_free_pad(void *program)
{
    if (!program)
        return -1;
    return FN(A_HPSS_FREE_PAD, int (*)(void *))(program);
}

/* Count empty pads with the same two checks the native finder uses: a null pad
 * state, or a pad state whose layers are all empty. */
static int hpss_free_count(void *program)
{
    uintptr_t lo = 0, hi = 0;
    if (!program || !rd((uintptr_t)program + 0x2808, &lo, 8) ||
        !rd((uintptr_t)program + 0x2810, &hi, 8) || !lo || hi < lo)
        return 0;
    int count = 0;
    size_t n = (size_t)((hi - lo) / 8);
    if (n > 128)
        n = 128;
    for (size_t i = 0; i < n; ++i) {
        uintptr_t st = 0;
        if (!rd(lo + i * 8, &st, 8))
            break;
        if (!st || !FN(A_HPSS_PAD_BUSY, int (*)(void *))((void *)st))
            count++;
    }
    return count;
}

/* Run one native Extract over source frames [start,end).  If `buf` is non-null
 * the writer callback substitutes it; otherwise the native reader copies the
 * source range itself (used by Spectral Chop). */
static int hpss_extract_ctx(void *screen, uintptr_t state, const float *buf,
                            unsigned start, unsigned end, unsigned source_frames,
                            unsigned buffer_frames, unsigned ch, int name,
                            int *dest)
{
    if (dest)
        *dest = -1;
    uintptr_t program = 0;
    if (!screen || start >= end || !ch ||
        !rd(state + 0x2310, &program, 8) || !program)
        return 0;
    int before = hpss_free_pad((void *)program);
    if (before < 0)
        return 0;
    if (dest)
        *dest = before;

    g_hpss_cmd_n = 0;
    g_hpss_arm_source = state;
    g_hpss_arm_buf = buf;
    g_hpss_arm_frames = buf ? buffer_frames : end - start;
    g_hpss_arm_source_frames = source_frames;
    g_hpss_arm_start = start;
    g_hpss_arm_end = end;
    g_hpss_arm_ch = ch;
    g_hpss_arm_name = name;
    g_hpss_arm = 1;
    FN(A_HPSS_EXTRACT_HANDLER, void (*)(void *))(screen);
    g_hpss_arm = 0;

    /* The command may be executed on an executor thread after the handler
     * returns.  Wait for the pad to change before starting the next one. */
    int after = before;
    for (int i = 0; i < 3000 && after == before; ++i) {
        usleep(1000);
        after = hpss_free_pad((void *)program);
    }
    int write_ok = buf ? g_hpss_write_ok : 1;
    int ok = after != before && write_ok;
    mlog("HPSS extract: pad %d -> free %d (%s, write %s) range %u..%u\n", before,
         after, after != before ? "assigned" : "NO ASSIGNMENT",
         write_ok ? "complete" : "INCOMPLETE", start, end);
    return ok;
}

static int bass_source_backup(const char *path, off_t *data_offset,
                              unsigned char **data, size_t *data_size)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    unsigned char header[12];
    int ok = 0;
    if (pread(fd, header, sizeof header, 0) != (ssize_t)sizeof header ||
        memcmp(header, "RIFF", 4) || memcmp(header + 8, "WAVE", 4))
        goto done;
    off_t pos = 12;
    for (int chunks = 0; chunks < 64; ++chunks) {
        unsigned char chunk[8];
        if (pread(fd, chunk, sizeof chunk, pos) != (ssize_t)sizeof chunk)
            break;
        uint32_t size = (uint32_t)chunk[4] | (uint32_t)chunk[5] << 8 |
                        (uint32_t)chunk[6] << 16 | (uint32_t)chunk[7] << 24;
        if (!memcmp(chunk, "data", 4)) {
            unsigned char *copy = malloc(size ? size : 1);
            if (!copy)
                break;
            if (pread(fd, copy, size, pos + 8) != (ssize_t)size) {
                free(copy);
                break;
            }
            *data_offset = pos + 8;
            *data = copy;
            *data_size = size;
            ok = 1;
            break;
        }
        pos += 8 + (off_t)size + (size & 1);
    }
done:
    close(fd);
    return ok;
}

static int bass_source_restore(const char *path, off_t data_offset,
                               const unsigned char *data, size_t data_size)
{
    int fd = open(path, O_WRONLY);
    if (fd < 0)
        return 0;
    size_t written = 0;
    while (written < data_size) {
        ssize_t n = pwrite(fd, data + written, data_size - written,
                           data_offset + (off_t)written);
        if (n <= 0) {
            close(fd);
            return 0;
        }
        written += (size_t)n;
    }
    int ok = fdatasync(fd) == 0;
    close(fd);
    return ok;
}

/* ------------------------------------------------------------ HPSS job/menu */
struct hpss_job {
    void *screen;
    uintptr_t state;
    uintptr_t sample;
    unsigned frames;
    unsigned ch;
    unsigned rate;
    float *input;
    float *harmonic;
    float *percussive;
    float *rendered;
    size_t rendered_frames;
    unsigned rendered_ch;
    int action;
    int rc;
    char source_path[512];
    off_t source_data_offset;
    unsigned char *source_audio_backup;
    size_t source_audio_bytes;
};

static struct hpss_job *volatile g_hpss_job;
static volatile int g_hpss_done;
static volatile int g_hpss_finalizing;
static volatile int g_hpss_data_down;
static char g_hpss_status[96];
static unsigned char g_hpss_btn[16];
static int g_hpss_btn_valid;
static int g_hpss_chop_before;
static int g_hpss_restore_pending;
static void hpss_set_status(const char *text);

static const char *sample_action_name(int action)
{
    switch (action) {
    case 1: return "Reverb";
    case 2: return "Vinyl";
    case 3: return "Lo-Fi";
    case 4: return "Bass Synth";
    default: return "HPSS";
    }
}

static void sample_action_status(int action, const char *message)
{
    char text[96];
    snprintf(text, sizeof text, "%s: %s", sample_action_name(action), message);
    hpss_set_status(text);
}

static void hpss_repaint(void)
{
    void *comp = g_paint_comp ? g_paint_comp : g_screen;
    if (orig_repaint && comp)
        orig_repaint(comp, 0, ((uint64_t)2000 << 32) | (uint64_t)2000);
}

static int sample_page_visible(void)
{
    void *s = g_screen;
    return s && FIELD(s, 0, uintptr_t) == base + A_SCREEN_VT &&
           FN(0x112fd50, int (*)(void *))(s) != 0;
}

static int hpss_chop_mode(void)
{
    void *s = g_screen;
    if (!s || FIELD(s, 0, uintptr_t) != base + A_SCREEN_VT)
        return 0;
    return FN(0x112fd50, int (*)(void *))((char *)s + 0x1ee0) != 0;
}

/* Stock Extract can leave the Sample page in Chop mode.  Replay the CHOP
 * button through the app's own dispatcher so the page returns to the mode the
 * user had before HPSS. */
static void hpss_restore_chop(void)
{
    if (!g_hpss_btn_valid || !orig_buttons || g_shift || !sample_page_visible()) {
        g_hpss_restore_pending = 1;
        return;
    }
    if (hpss_chop_mode() == g_hpss_chop_before)
        return;
    unsigned char ev[16];
    memcpy(ev, g_hpss_btn, sizeof ev);
    ev[8] = 0x4b; /* CHOP */
    ev[9] = 0x7f;
    orig_buttons(ev);
    ev[9] = 0x00;
    orig_buttons(ev);
    mlog("HPSS: restored chop mode to %d\n", g_hpss_chop_before);
}

static void *hpss_worker(void *arg)
{
    struct hpss_job *job = arg;
    int rc;
    if (job->action == 4)
        rc = bass_sample_render(job->rate, g_bass_knob[0], g_bass_knob[1],
                                g_bass_knob[2], &job->rendered,
                                &job->rendered_frames);
    else if (job->action != 0)
        rc = h3m_effect_render(base, (enum h3m_effect)(job->action - 1),
                               job->input, job->frames, job->ch, job->rate,
                               &job->rendered, &job->rendered_frames);
    else
        rc = hpss_split(job->input, job->frames, job->ch, job->rate,
                        &job->harmonic, &job->percussive);
    if (job->action == 4)
        job->rendered_ch = 2;
    job->rc = rc;
    __atomic_store_n(&g_hpss_done, 1, __ATOMIC_RELEASE);
    mlog("%s worker: rc=%d frames=%u ch=%u output=%zu\n",
         sample_action_name(job->action), rc, job->frames, job->ch,
         job->rendered_frames);
    hpss_repaint();
    return NULL;
}

static void hpss_free_job(struct hpss_job *job)
{
    if (!job)
        return;
    free(job->input);
    free(job->harmonic);
    free(job->percussive);
    free(job->rendered);
    free(job->source_audio_backup);
    free(job);
}

static void hpss_set_status(const char *text)
{
    if (!text)
        text = "";
    size_t n = strlen(text);
    if (n >= sizeof g_hpss_status)
        n = sizeof g_hpss_status - 1;
    memcpy(g_hpss_status, text, n);
    g_hpss_status[n] = 0;
}

static void sample_transform_start(int action)
{
    if (g_hpss_busy || g_hpss_job)
        return;
    void *screen = 0;
    uintptr_t state = 0, sample = 0;
    unsigned frames = 0, ch = 0, rate = 0;
    const char *name = sample_action_name(action);
    if (!hpss_sample_now(&screen, &state, &sample, &frames, &ch, &rate)) {
        sample_action_status(action, "no sample on this pad");
        toast(action ? "Effect: no sample" : "HPSS: no sample");
        return;
    }
    char path[512] = {0};
    if (!hpss_path_for(sample, frames, ch, rate, path, sizeof path)) {
        sample_action_status(action, "no exact WAV file");
        toast(action ? "Effect: no WAV" : "HPSS: no WAV");
        return;
    }
    float *input = NULL;
    if (!wav_read_f32(path, frames, ch, rate, &input, NULL, NULL)) {
        sample_action_status(action, "cannot decode WAV");
        toast(action ? "Effect: bad WAV" : "HPSS: bad WAV");
        return;
    }
    struct hpss_job *job = calloc(1, sizeof *job);
    if (!job) {
        free(input);
        sample_action_status(action, "out of memory");
        return;
    }
    job->screen = screen;
    job->state = state;
    job->sample = sample;
    job->frames = frames;
    job->ch = ch;
    job->rate = rate;
    job->input = input;
    job->action = action;
    job->rc = -1;
    g_hpss_chop_before = hpss_chop_mode();
    g_hpss_job = job;
    g_hpss_busy = 1;
    __atomic_store_n(&g_hpss_done, 0, __ATOMIC_RELEASE);
    char status[96];
    snprintf(status, sizeof status, "Processing %s...", name);
    hpss_set_status(status);
    mlog("%s start: %s frames=%u ch=%u rate=%u\n",
         name, path, frames, ch, rate);
    pthread_t th;
    int cr = pthread_create(&th, 0, hpss_worker, job);
    if (cr != 0) {
        g_hpss_job = NULL;
        g_hpss_busy = 0;
        hpss_free_job(job);
        sample_action_status(action, "worker failed");
        toast(action ? "Effect: worker failed" : "HPSS: worker failed");
        return;
    }
    pthread_detach(th);
    hpss_repaint();
}

static void hpss_start(void) { sample_transform_start(0); }
static void effect_start(int action) { sample_transform_start(action); }

static void fmbass_start(void)
{
    if (g_hpss_busy || g_hpss_job)
        return;
    void *screen = 0;
    uintptr_t state = 0, sample = 0;
    unsigned frames = 0, ch = 0, rate = 0;
    if (!hpss_sample_now(&screen, &state, &sample, &frames, &ch, &rate)) {
        sample_action_status(4, "load a sample pad first");
        toast("Bass: select sample pad");
        return;
    }
    struct hpss_job *job = calloc(1, sizeof *job);
    if (!job) {
        sample_action_status(4, "out of memory");
        return;
    }
    if (!hpss_path_for(sample, frames, ch, rate, job->source_path,
                       sizeof job->source_path) ||
        !bass_source_backup(job->source_path, &job->source_data_offset,
                            &job->source_audio_backup, &job->source_audio_bytes)) {
        hpss_free_job(job);
        sample_action_status(4, "cannot preserve source sample");
        toast("Bass: source read failed");
        return;
    }
    uintptr_t program = 0;
    if (!rd(state + 0x2310, &program, 8) || !program || hpss_free_count((void *)program) < 1) {
        hpss_free_job(job);
        sample_action_status(4, "need 1 empty pad");
        toast("Bass: need 1 pad");
        return;
    }
    job->screen = screen;
    job->state = state;
    job->sample = sample;
    job->frames = frames;
    job->ch = ch;
    job->rate = rate;
    job->action = 4;
    job->rendered_ch = 2;
    job->rc = -1;
    g_hpss_chop_before = hpss_chop_mode();
    bass_preview_set_active(0);
    g_hpss_job = job;
    g_hpss_busy = 1;
    __atomic_store_n(&g_hpss_done, 0, __ATOMIC_RELEASE);
    hpss_set_status("Rendering Bass C2...");
    pthread_t th;
    if (pthread_create(&th, 0, hpss_worker, job) != 0) {
        g_hpss_job = NULL;
        g_hpss_busy = 0;
        hpss_free_job(job);
        bass_preview_set_params(g_bass_knob[0], g_bass_knob[1], g_bass_knob[2]);
        bass_preview_set_active(1);
        sample_action_status(4, "worker failed");
        return;
    }
    pthread_detach(th);
    hpss_repaint();
}

static void hpss_close(void);
static void cc_replay_k3(unsigned raw);

/* ------------------------------------------------------- spectral markers */
/* The input hook is NOT the UI thread. Publish only a request there; all
 * model calls below run from screen_paint, after the stock paint returns. */
static unsigned char g_spectral_event[16];
static struct {
    int phase, n;
    void *screen;
    uintptr_t state, sample;
    unsigned frames, ch, rate, pos[16];
    long long deadline;
} g_spectral;

static unsigned slice_count_of(uintptr_t sample, uintptr_t *begin_out,
                               uintptr_t *cont_out)
{
    uintptr_t buf = 0, begin = 0, end = 0, cap = 0;
    if (!sample || !rd(sample + 0x130, &buf, 8) || !buf ||
        !rd(buf + 0x70, &begin, 8) || !rd(buf + 0x78, &end, 8) ||
        !rd(buf + 0x80, &cap, 8) || end < begin || cap < end ||
        ((end - begin) & 7) || (end - begin) / 8 > 128)
        return 0;
    if (begin_out) *begin_out = begin;
    if (cont_out) *cont_out = buf + 0x10;
    return (unsigned)((end - begin) / 8);
}

static void spectral_finish(const char *error)
{
    char status[96];
    if (error)
        snprintf(status, sizeof status, "Spectral: %s", error);
    else
        snprintf(status, sizeof status, "Spectral: %d slices on this pad", g_spectral.n);
    mlog("%s\n", status);
    hpss_set_status(status);
    toast(error ? status : "Spectral: chopped");
    g_spectral.phase = 0;
    g_hpss_busy = 0;
    __atomic_store_n(&g_spectral_active, 0, __ATOMIC_RELEASE);
    hpss_repaint();
}

/* Same binding path used by the stock Split handler (Ghidra 0x1756728).
 * Manual is enum 0. Calling the binding's model callback preserves the pad's
 * chop type; assigning the view's cached enum alone does not. */
static int spectral_manual(void)
{
    uintptr_t binding = 0, owner = 0, invoke = 0, manager = 0;
    unsigned char flags = 0;
    if (!rd((uintptr_t)g_screen + 0x168, &binding, 8) || !binding ||
        !rd(binding + 0xe8, &flags, 1) || !rd(binding + 0xe0, &owner, 8))
        return 0;
    if (!(flags & 1) && owner) {
        if (!rd(binding + 0xd0, &manager, 8) || !manager ||
            !rd(binding + 0xd8, &invoke, 8) || !invoke)
            return 0;
        int manual = 0;
        ((void (*)(uintptr_t, uintptr_t, const int *))invoke)(binding + 0xc0, owner, &manual);
    } else {
        FN(0x15d2840, void (*)(uintptr_t, int))(binding + 0x80, 0);
    }
    return 1;
}

static int spectral_verify(void)
{
    uintptr_t begin = 0;
    if (slice_count_of(g_spectral.sample, &begin, 0) != (unsigned)g_spectral.n)
        return 0;
    for (int i = 0; i < g_spectral.n; ++i) {
        uintptr_t slice = 0, start = 0, end = 0;
        unsigned want_end = i + 1 < g_spectral.n ? g_spectral.pos[i + 1] : g_spectral.frames;
        if (!rd(begin + (uintptr_t)i * 8, &slice, 8) || !slice ||
            !rd(slice + 0x78, &start, 8) || !rd(slice + 0xd0, &end, 8) ||
            start != g_spectral.pos[i] || end != want_end)
            return 0;
    }
    return 1;
}

static int spectral_write_markers(void)
{
    uintptr_t cont = 0, selection = 0;
    slice_count_of(g_spectral.sample, 0, &cont);
    if (!cont || !rd((uintptr_t)g_screen + 0x180, &selection, 8) || !selection)
        return 0;
    /* Native Reset and Insert, as called by the stock Chop UI. These own the
     * Slice allocation, destruction, listener links, selection and broadcasts.
     * Never shorten its vector or poke observable Slice fields ourselves. */
    FN(0x2950aa0, void (*)(uintptr_t))(cont);
    for (int i = 0; i < g_spectral.n; ++i) {
        int index = FN(0x2950d20, int (*)(uintptr_t, unsigned, unsigned, unsigned))(
            cont, g_spectral.pos[i], 1, g_spectral.frames);
        if (index != i)
            return 0;
    }
    /* Reconcile adjacent ends through the native setters, including the tail. */
    FN(0x2950bb0, void (*)(uintptr_t, unsigned))(cont, g_spectral.frames);
    int first = 0;
    FN(0x16680a0, void (*)(uintptr_t, const int *))(selection + 0x618, &first);
    return spectral_verify();
}

static void spectral_start(void)
{
    int expected = 0;
    if (g_hpss_busy || g_hpss_job ||
        !__atomic_compare_exchange_n(&g_spectral_active, &expected, -1, 0,
                                     __ATOMIC_RELEASE, __ATOMIC_RELAXED))
        return;
    memcpy(g_spectral_event, g_hpss_btn, sizeof g_spectral_event);
    __atomic_store_n(&g_spectral_active, 1, __ATOMIC_RELEASE);
    hpss_repaint();
}

/* One phase per UI pass: a CHOP press schedules a page switch, so it cannot
 * be followed by a sleep/poll or by another toggle in the same callback. */
static void spectral_tick(void)
{
    if (__atomic_load_n(&g_spectral_active, __ATOMIC_ACQUIRE) != 1)
        return;
    if (!g_spectral.phase) {
        g_hpss_menu = 0;
        g_hpss_busy = 1;
        if (!hpss_sample_now(&g_spectral.screen, &g_spectral.state, &g_spectral.sample,
                             &g_spectral.frames, &g_spectral.ch, &g_spectral.rate)) {
            spectral_finish("no sample on this pad");
            return;
        }
        char path[512] = {0};
        float *input = NULL;
        if (!hpss_path_for(g_spectral.sample, g_spectral.frames, g_spectral.ch,
                           g_spectral.rate, path, sizeof path) ||
            !wav_read_f32(path, g_spectral.frames, g_spectral.ch, g_spectral.rate,
                          &input, NULL, NULL)) {
            spectral_finish("cannot read sample WAV");
            return;
        }
        unsigned detected = 0;
        g_spectral.n = spectral_onsets(input, g_spectral.frames, g_spectral.ch,
                                       g_spectral.rate, g_spectral.pos, 16, &detected);
        free(input);
        if (g_spectral.n < 2) {
            spectral_finish("no onsets");
            return;
        }
        for (int i = 0; i < g_spectral.n; ++i) {
            if (g_spectral.pos[i] >= g_spectral.frames ||
                (i && g_spectral.pos[i] <= g_spectral.pos[i - 1])) {
                spectral_finish("invalid onsets");
                return;
            }
        }
        mlog("Spectral: source=%p frames=%u onsets=%d detected=%u\n",
             (void *)g_spectral.sample, g_spectral.frames, g_spectral.n, detected);
        if (!hpss_chop_mode() && orig_buttons) {
            g_spectral_event[8] = 0x4b;
            g_spectral_event[9] = 0x7f;
            orig_buttons(g_spectral_event);
            g_spectral_event[9] = 0;
            orig_buttons(g_spectral_event);
        }
        if (!spectral_manual()) {
            spectral_finish("cannot select Manual chop");
            return;
        }
        g_spectral.phase = 1;
        g_spectral.deadline = now_ms() + 3000;
    } else {
        void *screen = 0;
        uintptr_t state = 0, sample = 0;
        unsigned frames = 0, ch = 0, rate = 0;
        if (!hpss_sample_now(&screen, &state, &sample, &frames, &ch, &rate) ||
            screen != g_spectral.screen || state != g_spectral.state ||
            sample != g_spectral.sample || frames != g_spectral.frames ||
            ch != g_spectral.ch || rate != g_spectral.rate) {
            spectral_finish("sample changed; cancelled");
            return;
        }
        uintptr_t binding = 0;
        int type = -1;
        rd((uintptr_t)screen + 0x168, &binding, 8);
        if (binding) rd(binding + 0x9c, &type, 4);
        if (!hpss_chop_mode() || type != 0) {
            if (now_ms() >= g_spectral.deadline)
                spectral_finish("Manual chop did not open");
        } else if (g_spectral.phase == 1) {
            if (!spectral_write_markers()) {
                spectral_finish("native marker write failed");
                return;
            }
            g_spectral.phase = 2;
            g_spectral.deadline = now_ms() + 150;
        } else if (now_ms() >= g_spectral.deadline) {
            spectral_finish(spectral_verify() ? NULL : "marker verification failed");
        }
    }
    hpss_repaint();
}

#if MINIMOD_DEBUG
/* ------------------------------------------------------- chop probe (debug) */
/* All addresses below are Ghidra VAs minus 0x100000 (the mod's own offset
 * space). Mixing the two spaces is how the first version of this probe ended
 * up hooking unrelated functions and calling garbage. */
/* Stock slice editing, found through the "Insert Slice" undo label at
 * 0x46a80e0 and the telemetry key "slices_added":
 *   FUN_021a3b10(editor, {sample, refcounted}, frame)  add a Manual slice
 *   FUN_021a9fa0(editor)                              add at the playhead
 *   FUN_01a60af0(ctrl, {sample, refcounted}, v, ed)   the undoable command
 * The probe only observes; the apply path is written once these are verified. */
static void (*orig_add_slice)(void *, void *, uint64_t);
static void (*orig_add_slice_ph)(void *);
static void (*orig_insert_cmd)(void *, void *, unsigned, void *);
static void (*orig_rmall_cmd)(void *, void *, void *);
static uintptr_t g_chop_editor;


/* Slice objects are ~0x350-byte heap allocations, so the chop code that makes
 * them can be found from the allocation's return address. Debug only; the
 * logger is off unless the probe enables it. */
static void *(*g_real_malloc)(size_t);
static volatile int g_malloc_probe;
static volatile int g_malloc_n;

void *malloc(size_t n)
{
    if (!g_real_malloc)
        g_real_malloc = (void *(*)(size_t))dlsym(RTLD_NEXT, "malloc");
    if (!g_real_malloc)
        return 0;
    void *p = g_real_malloc(n);
    if (g_malloc_probe && n >= 0x100 && n <= 0x8000 && p && g_malloc_n < 400) {
        g_malloc_n++;
        mlog("chop: malloc %zu -> %p lr=%p\n", n, p, __builtin_return_address(0));
    }
    return p;
}

static void cc_replay(unsigned cc, unsigned raw);

static unsigned chop_slice_count(uintptr_t sample)
{
    uintptr_t buf = 0, lo = 0, hi = 0;
    if (!sample || !rd(sample + 0x130, &buf, 8) || !buf)
        return 0;
    if (!rd(buf + 0x10 + 0x60, &lo, 8) || !rd(buf + 0x10 + 0x68, &hi, 8) || hi < lo)
        return 0;
    return (unsigned)((hi - lo) >> 3);
}

static uintptr_t chop_sample_from_sel(void *sel)
{
    uintptr_t sample = 0;
    if (sel)
        rd((uintptr_t)sel, &sample, 8);
    return sample;
}

/* The serialized layer object keeps the chop model: SlicingStyle at +0xa8 and
 * four std::vector<SlicePoint> pairs (each 0x18 bytes) at +0xb0/+0xc8/+0xe0/
 * +0xf8. Print them so the live layout can be checked against the parser. */
static void chop_dump_layer(uintptr_t lay)
{
    unsigned style = 0;
    uintptr_t pairs[4][3] = {{0}};
    if (!lay)
        return;
    rd(lay + 0xa8, &style, 4);
    mlog("chop: layer=%p style=%u (+0xa8)\n", (void *)lay, style);
    {
        unsigned char f6e0 = 0;
        unsigned m418 = 0, n738 = 0, a6e4 = 0;
        rd(lay + 0x6e0, &f6e0, 1);
        rd(lay + 0x418, &m418, 4);
        rd(lay + 0x738, &n738, 4);
        rd(lay + 0x6e4, &a6e4, 4);
        mlog("chop:   flags +0x6e0=%u marker +0x418=%u +0x738=%u +0x6e4=%u\n",
             f6e0, m418, n738, a6e4);
    }
    for (int i = 0; i < 4; ++i) {
        uintptr_t at = lay + 0xb0 + (uintptr_t)i * 0x18;
        rd(at, &pairs[i][0], 8);
        rd(at + 8, &pairs[i][1], 8);
        rd(at + 0x10, &pairs[i][2], 8);
        mlog("chop:   vec[%d] at +0x%lx begin=%p end=%p cap=%p n=%lld\n", i,
             (unsigned long)(0xb0 + i * 0x18), (void *)pairs[i][0],
             (void *)pairs[i][1], (void *)pairs[i][2],
             (long long)((pairs[i][1] - pairs[i][0]) / 0x18));
    }
    for (int i = 0; i < 4; ++i) {
        uintptr_t n = (pairs[i][1] - pairs[i][0]) / 0x18;
        if (n > 64)
            n = 64;
        for (uintptr_t k = 0; k < n; ++k) {
            uintptr_t at = pairs[i][0] + k * 0x18;
            double t = 0, e = 0;
            float f = 0;
            rd(at, &t, 8);
            rd(at + 8, &f, 4);
            rd(at + 0x10, &e, 8);
            mlog("chop:   vec[%d][%lu] t=%.6f f=%.4f e=%.4f\n", i,
                 (unsigned long)k, t, (double)f, e);
        }
    }
}

/* Find the editor object the stock slice commands take: an object whose
 * +0x4c8 points at the live Sample. Discovery only - the shipped path must not
 * depend on a full-heap search. */
static void chop_scan(uintptr_t sample)
{
    if (!sample)
        return;
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f)
        return;
    size_t chunk = 8u << 20;
    unsigned char *buf = malloc(chunk);
    if (!buf) {
        fclose(f);
        return;
    }
    char line[512];
    int hits = 0;
    while (fgets(line, sizeof line, f) && hits < 32) {
        unsigned long lo = 0, hi = 0;
        char perms[8] = {0};
        if (sscanf(line, "%lx-%lx %7s", &lo, &hi, perms) != 3 || perms[1] != 'w')
            continue;
        for (unsigned long a = lo; a < hi && hits < 32; a += chunk) {
            size_t n = (hi - a) < chunk ? (size_t)(hi - a) : chunk;
            if (!rd(a, buf, n))
                continue;
            for (size_t o = 0; o + 8 <= n && hits < 32; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q != sample || a + o < 0x4c8)
                    continue;
                uintptr_t base = a + o - 0x4c8;
                uintptr_t ctl = 0, sel = 0, mgr = 0, mode = 0;
                unsigned ref = 0;
                rd(base + 0x10c8, &ctl, 8);
                rd(base + 0x4d0, &sel, 8);
                if (ctl)
                    rd(ctl + 0x60, &mgr, 8);
                rd(base + 0x528, &mode, 8);
                if (sel)
                    rd(sel + 8, &ref, 4);
                /* The editor's selection is a heap pointer and its controller
                 * has a command manager; that rejects the text and table
                 * objects whose +0x4c8 happens to equal the sample. */
                int shapecand = (sel >> 40) == 0xffff && ctl && mgr;
                if (!shapecand)
                    continue;
                mlog("chop: scan cand editor=%p ctl=%p mgr=%p sel=%p ref=%u mode=%llu\n",
                     (void *)base, (void *)ctl, (void *)mgr, (void *)sel, ref,
                     (unsigned long long)(mode & 0xffffffff));
                hits++;
            }
        }
    }
    free(buf);
    fclose(f);
    mlog("chop: scan done, %d candidate(s)\n", hits);
}

static void chop_dump(const char *tag, uintptr_t sample)
{
    uintptr_t buf = 0, lo = 0, hi = 0;
    unsigned n = chop_slice_count(sample);
    mlog("chop: %s sample=%p slices=%u\n", tag, (void *)sample, n);
    if (!sample || !rd(sample + 0x130, &buf, 8) || !buf)
        return;
    if (!rd(buf + 0x10 + 0x60, &lo, 8) || !rd(buf + 0x10 + 0x68, &hi, 8))
        return;
    for (unsigned i = 0; i < n && i < 32; ++i) {
        uintptr_t el = 0;
        if (!rd(lo + i * 8, &el, 8) || !el)
            break;
        uintptr_t d0 = 0, c8 = 0, d8 = 0;
        rd(el + 0xc8, &c8, 8);
        rd(el + 0xd0, &d0, 8);
        rd(el + 0xd8, &d8, 8);
        mlog("chop:   slice[%u] obj=%p +0xc8=%llu +0xd0=%llu +0xd8=%llu\n", i,
             (void *)el, (unsigned long long)c8, (unsigned long long)d0,
             (unsigned long long)d8);
    }
}

static void add_slice_hook(void *editor, void *sel, uint64_t pos)
{
    uintptr_t sample = chop_sample_from_sel(sel);
    g_chop_editor = (uintptr_t)editor;
    mlog("chop: add_slice editor=%p screen=%p pos=%llu before=%u\n", editor,
         g_screen, (unsigned long long)pos, chop_slice_count(sample));
    if (orig_add_slice)
        orig_add_slice(editor, sel, pos);
    mlog("chop: add_slice after=%u\n", chop_slice_count(sample));
}

static void add_slice_ph_hook(void *editor)
{
    g_chop_editor = (uintptr_t)editor;
    mlog("chop: add_slice_ph editor=%p screen=%p\n", editor, g_screen);
    if (orig_add_slice_ph)
        orig_add_slice_ph(editor);
}

static void insert_cmd_hook(void *ctl, void *sel, unsigned v, void *ed)
{
    mlog("chop: insert_cmd ctl=%p sel=%p val=%u ed=%p editor_seen=%p\n", ctl, sel,
         v, ed, (void *)g_chop_editor);
    if (orig_insert_cmd)
        orig_insert_cmd(ctl, sel, v, ed);
}

/* The slice editor owns the selected sample at +0x4c8, the marker index at
 * +0x528 and the refcounted selection at +0x4d0. Any of its methods hands us
 * the instance without guessing. */
static void (*orig_slice_sel)(void *, unsigned);
static void slice_sel_hook(void *editor, unsigned index)
{
    if (!g_chop_editor) {
        g_chop_editor = (uintptr_t)editor;
        mlog("chop: editor captured from selectSlice editor=%p index=%u\n", editor,
             index);
    }
    if (orig_slice_sel)
        orig_slice_sel(editor, index);
}

/* Remove Slice: (controller, {sample, refcounted}, value, editor). Its fourth
 * argument is the slice editor itself. */
static void (*orig_rm_slice)(void *, void *, unsigned, void *);
static void rm_slice_hook(void *ctl, void *sel, unsigned v, void *ed)
{
    uintptr_t sample = chop_sample_from_sel(sel);
    mlog("chop: rm_slice ctl=%p sel=%p val=%u ed=%p sample=%p before=%u\n", ctl, sel, v,
         ed, (void *)sample, chop_slice_count(sample));
    if (ed) {
        g_chop_editor = (uintptr_t)ed;
        mlog("chop: editor captured from rm_slice ed=%p\n", ed);
    }
    if (orig_rm_slice)
        orig_rm_slice(ctl, sel, v, ed);
    mlog("chop: rm_slice after=%u\n", chop_slice_count(sample));
}

/* The layer's marker resolver is called by every chop UI action. Its caller
 * identifies the editor-side function; hooking that entry then yields the
 * editor itself in x0. */
static int (*orig_marker_at)(void *, unsigned);
static void marker_at_hook(void *layer, unsigned index)
{
    uintptr_t lr = (uintptr_t)__builtin_return_address(0);
    mlog("chop: marker_at layer=%p index=%u caller=0x%llx\n", layer, index,
         (unsigned long long)(lr - base));
    if (orig_marker_at)
        orig_marker_at(layer, index);
}

/* Slice-list helper that runs on every edit; its caller is the edit code. */
static void (*orig_slice_publish)(void *);
static void slice_publish_hook(void *a)
{
    uintptr_t lr = (uintptr_t)__builtin_return_address(0);
    mlog("chop: slice_publish arg=%p caller=0x%llx\n", a,
         (unsigned long long)(lr - base));
    if (orig_slice_publish)
        orig_slice_publish(a);
}


/* ---- snapshot: what the UI just changed ---------------------------------- */
static int g_snap_track;
static int g_snap_n;

static unsigned fnv(const unsigned char *p, size_t n)
{
    unsigned h = 2166136261u;
    for (size_t i = 0; i < n; ++i) {
        h ^= p[i];
        h *= 16777619u;
    }
    return h;
}

static void chop_snapshot(const char *tag)
{
    void *screen = 0;
    uintptr_t state = 0, sample = 0;
    unsigned frames = 0, ch = 0, rate = 0;
    if (!hpss_sample_now(&screen, &state, &sample, &frames, &ch, &rate))
        return;
    uintptr_t binding = 0, st = 0, lay = 0;
    if (screen && rd((uintptr_t)screen + 0x178, &binding, 8) && binding &&
        rd(binding + 0x60, &st, 8) && st && rd(st + 0x22e8, &lay, 8) && lay)
        (void)lay;
    unsigned marker = 0, count = chop_slice_count(sample);
    if (lay)
        rd(lay + 0x418, &marker, 4);
    mlog("SNAP %s chop=%d marker=%u count=%u frames=%u sample=%p layer=%p\n", tag,
         in_chop_mode(), marker, count, frames, (void *)sample, (void *)lay);
    if (lay) {
        static const unsigned lo[] = {0xa8, 0xb0, 0xb8, 0xc0, 0xc8, 0xd0, 0xd8, 0xe0,
                                      0xe8, 0xf0, 0xf8, 0x100, 0x6e0, 0x738};
        for (unsigned i = 0; i < sizeof lo / sizeof lo[0]; ++i) {
            uintptr_t q = 0;
            rd(lay + lo[i], &q, 8);
            mlog("SNAP L %03x %016llx\n", lo[i], (unsigned long long)q);
        }
    }
    uintptr_t buf = 0, lo = 0, hi = 0;
    if (sample && rd(sample + 0x130, &buf, 8) && buf &&
        rd(buf + 0x70, &lo, 8) && rd(buf + 0x78, &hi, 8) && hi > lo) {
        unsigned n = (unsigned)((hi - lo) / 8);
        mlog("SNAP slicevec %p..%p n=%u\n", (void *)lo, (void *)hi, n);
        if (n > 8)
            n = 8;
        for (unsigned i = 0; i < n; ++i) {
            uintptr_t sp = 0;
            if (!rd(lo + i * 8, &sp, 8) || !sp)
                break;
            unsigned char obj[0x340];
            memset(obj, 0, sizeof obj);
            rd(sp, obj, sizeof obj);
            mlog("SNAP P %u %p hash=%08x\n", i, (void *)sp, fnv(obj, sizeof obj));
            for (unsigned k = 0; k < 0x140; k += 8) {
                uintptr_t q = 0;
                rd(sp + k, &q, 8);
                mlog("SNAP S %u %03x %016llx\n", i, k, (unsigned long long)q);
            }
        }
    }
}

/* Every slice edit asks the sample for its slice list first, so the caller of
 * this accessor during a merge/erase is the code that mutates it. */
static void *(*orig_slice_list)(void *);
static void *slice_list_hook(void *sample)
{
    uintptr_t lr = (uintptr_t)__builtin_return_address(0);
    mlog("chop: slice_list sample=%p caller=0x%llx\n", sample,
         (unsigned long long)(lr - base));
    return orig_slice_list ? orig_slice_list(sample) : 0;
}

/* The UI's slice-action dispatch: (editor, msg, kind). kind 1 means "slice
 * edit"; the editor reads its selected marker from +0x528. */
static void (*orig_slice_action)(void *, void *, int);
static void slice_action_hook(void *ed, void *msg, int kind)
{
    uintptr_t sample = 0, ctl = 0, sel = 0;
    unsigned marker = 0, playhead = 0;
    rd((uintptr_t)ed + 0x4c8, &sample, 8);
    rd((uintptr_t)ed + 0x4d0, &sel, 8);
    rd((uintptr_t)ed + 0x10c8, &ctl, 8);
    rd((uintptr_t)ed + 0x528, &marker, 4);
    rd((uintptr_t)ed + 0x848, &playhead, 4);
    mlog("chop: slice_action ed=%p kind=%d sample=%p marker=%u playhead=%u ctl=%p "
         "sel=%p\n", ed, kind, (void *)sample, marker, playhead, (void *)ctl,
         (void *)sel);
    if (ed)
        g_chop_editor = (uintptr_t)ed;
    if (orig_slice_action)
        orig_slice_action(ed, msg, kind);
}

/* Split Slice: same shape; its fourth argument is the editor. */
static void (*orig_split_cmd)(void *, void *, unsigned, void *);
static void split_cmd_hook(void *ctl, void *sel, unsigned v, void *ed)
{
    uintptr_t sample = chop_sample_from_sel(sel);
    mlog("chop: split_cmd ctl=%p sel=%p val=%u ed=%p sample=%p before=%u\n", ctl, sel, v,
         ed, (void *)sample, chop_slice_count(sample));
    if (ed) {
        g_chop_editor = (uintptr_t)ed;
        mlog("chop: editor captured from split ed=%p\n", ed);
    }
    if (orig_split_cmd)
        orig_split_cmd(ctl, sel, v, ed);
    mlog("chop: split_cmd after=%u\n", chop_slice_count(sample));
}

static void rmall_cmd_hook(void *ctl, void *sel, void *ed)
{
    mlog("chop: rmall_cmd ctl=%p sel=%p ed=%p\n", ctl, sel, ed);
    if (orig_rmall_cmd)
        orig_rmall_cmd(ctl, sel, ed);
}

/* Probe calls go through a guarded boundary: a wrong candidate must not take
 * the app down while the layout is still being established. */
static volatile int g_chop_ok;

static void chop_add_guarded(uintptr_t ed, uintptr_t *pair, uint64_t frame)
{
    g_chop_ok = 0;
    if (sigsetjmp(g_cc_jmp, 1) == 0) {
        g_cc_recover = 1;
        if (orig_add_slice)
            orig_add_slice((void *)ed, pair, frame);
        g_cc_recover = 0;
        g_chop_ok = 1;
    } else {
        g_cc_recover = 0;
    }
}

static void chop_rmall_guarded(uintptr_t ctl, uintptr_t *pair, uintptr_t ed)
{
    g_chop_ok = 0;
    if (sigsetjmp(g_cc_jmp, 1) == 0) {
        g_cc_recover = 1;
        if (orig_rmall_cmd)
            orig_rmall_cmd((void *)ctl, pair, (void *)ed);
        g_cc_recover = 0;
        g_chop_ok = 1;
    } else {
        g_cc_recover = 0;
    }
}

/* Stronger signature: the editor holds the *selected marker index* at +0x528,
 * a Sample pointer at +0x4c8 and the command controller at +0x10c8. The marker
 * index is small and changes with the UI, so matching it is very selective. */
static void chop_scan_marker(uintptr_t layer)
{
    unsigned marker = 0;
    if (!layer || !rd(layer + 0x418, &marker, 4)) {
        mlog("chop: scan2 no marker\n");
        return;
    }
    mlog("chop: scan2 looking for marker=%u\n", marker);
    FILE *f = fopen("/proc/self/maps", "r");
    if (!f)
        return;
    size_t chunk = 8u << 20;
    unsigned char *buf = malloc(chunk);
    if (!buf) {
        fclose(f);
        return;
    }
    char line[512];
    int hits = 0;
    while (fgets(line, sizeof line, f) && hits < 16) {
        unsigned long lo = 0, hi = 0;
        char perms[8] = {0};
        if (sscanf(line, "%lx-%lx %7s", &lo, &hi, perms) != 3 || perms[1] != 'w')
            continue;
        for (unsigned long a = lo; a < hi && hits < 16; a += chunk) {
            size_t n = (hi - a) < chunk ? (size_t)(hi - a) : chunk;
            if (!rd(a, buf, n))
                continue;
            for (size_t o = 0; o + 4 <= n && hits < 16; o += 4) {
                unsigned m;
                memcpy(&m, buf + o, 4);
                if (m != marker || a + o < 0x528)
                    continue;
                uintptr_t b = a + o - 0x528;
                uintptr_t smp = 0, sel = 0, ctl = 0, mgr = 0, ph = 0, key = 0;
                rd(b + 0x4c8, &smp, 8);
                rd(b + 0x4d0, &sel, 8);
                rd(b + 0x10c8, &ctl, 8);
                rd(b + 0x848, &ph, 8);
                rd(b, &key, 8);
                if (ctl)
                    rd(ctl + 0x60, &mgr, 8);
                if (!ctl || !mgr || (ctl >> 40) != 0xffff || (mgr >> 40) != 0xffff)
                    continue;
                mlog("chop: scan2 cand base=%p vt=0x%llx sample=%p sel=%p ctl=%p "
                     "playhead=%llu\n", (void *)b, (unsigned long long)(key - base),
                     (void *)smp, (void *)sel, (void *)ctl,
                     (unsigned long long)(ph & 0xffffffff));
                hits++;
            }
        }
    }
    free(buf);
    fclose(f);
    mlog("chop: scan2 done, %d\n", hits);
}

/* The editor is a long-lived object that owns the selected Sample at +0x4c8.
 * Look for a pointer to it inside the objects we can already reach. */
static void chop_find(uintptr_t sample)
{
    uintptr_t roots[3] = {(uintptr_t)g_screen, 0, 0};
    uintptr_t binding = 0, st = 0;
    if (roots[0] && rd(roots[0] + 0x178, &binding, 8) && binding &&
        rd(binding + 0x60, &st, 8))
        roots[1] = st;
    roots[2] = binding;
    const char *names[3] = {"screen", "state", "binding"};
    for (int r = 0; r < 3; ++r) {
        if (!roots[r])
            continue;
        for (uintptr_t o = 0; o < 0x12000; o += 8) {
            uintptr_t v = 0;
            if (!rd(roots[r] + o, &v, 8) || !v)
                continue;
            if (v < 0xffff00000000ULL)
                continue;
            uintptr_t s = 0, ctl = 0, sel = 0;
            if (!rd(v + 0x4c8, &s, 8) || s != sample)
                continue;
            rd(v + 0x10c8, &ctl, 8);
            rd(v + 0x4d0, &sel, 8);
            mlog("chop: find hit %s+0x%lx -> editor=%p ctl=%p sel=%p\n", names[r],
                 (unsigned long)o, (void *)v, (void *)ctl, (void *)sel);
        }
    }
    mlog("chop: find done\n");
}

/* Commands are queued by the probe thread and run on the UI thread from the
 * button hook: the app's model is not safe to touch from a foreign thread. */
static char g_chop_cmd[64];

static void chop_probe_run(void)
{
    char cmd[64];
    memcpy(cmd, g_chop_cmd, sizeof cmd);
    if (!cmd[0])
        return;
    g_chop_cmd[0] = 0;
    void *screen = 0;
    uintptr_t state = 0, sample = 0;
    unsigned frames = 0, ch = 0, rate = 0;
    int have = hpss_sample_now(&screen, &state, &sample, &frames, &ch, &rate);
    mlog("chop: cmd '%s' editor=%p screen=%p sample=%p frames=%u\n", cmd,
         (void *)g_chop_editor, screen, (void *)sample, frames);
    mlog("chop: view chop=%d sample_page=%d levels=%d last_pad=%d shift=%d\n",
         in_chop_mode(), sample_page_visible(), levels_active(), g_last_pad,
         g_shift);
    if (!strncmp(cmd, "rw ", 3)) {
        unsigned long addr = 0, len = 0;
        if (sscanf(cmd + 3, "%lx %lx", &addr, &len) != 2 || !addr || len > 0x400)
            return;
        for (unsigned long o = 0; o < len; o += 8) {
            uintptr_t q = 0;
            if (!rd((uintptr_t)addr + o, &q, 8))
                break;
            mlog("chop: R %03lx %016llx\n", o, (unsigned long long)q);
        }
        return;
    }
    if (!strncmp(cmd, "sl ", 3)) {
        unsigned idx = (unsigned)strtoul(cmd + 3, 0, 10);
        uintptr_t buf = 0, lo = 0, hi = 0;
        if (!sample || !rd(sample + 0x130, &buf, 8) || !buf)
            return;
        lo = buf + 0x10 + 0x60;
        if (!rd(lo, &lo, 8) || !rd(buf + 0x10 + 0x68, &hi, 8) || hi <= lo)
            return;
        if (idx >= (hi - lo) / 8) {
            mlog("chop: sl idx %u out of range\n", idx);
            return;
        }
        uintptr_t sp = 0;
        if (!rd(lo + idx * 8, &sp, 8) || !sp)
            return;
        mlog("chop: sl idx=%u obj=%p\n", idx, (void *)sp);
        for (unsigned o = 0; o < 0x140; o += 8) {
            uintptr_t q = 0;
            if (!rd(sp + o, &q, 8))
                break;
            mlog("chop: S %03x %016llx\n", o, (unsigned long long)q);
        }
        return;
    }
    if (!strcmp(cmd, "lay")) {
        uintptr_t binding = 0, st = 0, lay = 0;
        if (!screen || !rd((uintptr_t)screen + 0x178, &binding, 8) || !binding ||
            !rd(binding + 0x60, &st, 8) || !st || !rd(st + 0x22e8, &lay, 8) || !lay) {
            mlog("chop: lay no layer\n");
            return;
        }
        mlog("chop: lay layer=%p\n", (void *)lay);
        for (unsigned o = 0; o < 0xb30; o += 8) {
            uintptr_t q = 0;
            if (!rd(lay + o, &q, 8))
                break;
            mlog("chop: L %03x %016llx\n", o, (unsigned long long)q);
        }
        return;
    }
    if (!strcmp(cmd, "scan2")) {
        uintptr_t binding = 0, st = 0, lay = 0;
        if (screen && rd((uintptr_t)screen + 0x178, &binding, 8) && binding &&
            rd(binding + 0x60, &st, 8) && st && rd(st + 0x22e8, &lay, 8))
            chop_scan_marker(lay);
        return;
    }
    if (!strcmp(cmd, "find")) {
        chop_find(sample);
        return;
    }
    if (!strcmp(cmd, "scan")) {
        chop_scan(sample);
        return;
    }
    if (!strcmp(cmd, "dump")) {
        chop_dump("before", sample);
        {
            uintptr_t binding = 0, st = 0, lay = 0, smp = 0;
            if (screen && rd((uintptr_t)screen + 0x178, &binding, 8) && binding &&
                rd(binding + 0x60, &st, 8) && st &&
                rd(st + 0x22e8, &lay, 8) && lay) {
                mlog("chop: chain screen=%p binding=%p state=%p layer=%p\n", screen,
                     (void *)binding, (void *)st, (void *)lay);
                if (rd(lay + 0x4a8, &smp, 8))
                    mlog("chop:   layer+0x4a8=%p (sample)\n", (void *)smp);
                chop_dump_layer(lay);
            }
            /* Which of the objects we can already reach is the editor that the
             * stock slice commands take as their first argument? */
            uintptr_t cand[4] = {(uintptr_t)screen, binding, st, lay};
            const char *names[4] = {"screen", "binding", "state", "layer"};
            for (int i = 0; i < 4; ++i) {
                if (!cand[i])
                    continue;
                uintptr_t c0 = 0, d0 = 0, ctl = 0, mode = 0, ph = 0;
                rd(cand[i] + 0x4c8, &c0, 8);
                rd(cand[i] + 0x4d0, &d0, 8);
                rd(cand[i] + 0x10c8, &ctl, 8);
                rd(cand[i] + 0x528, &mode, 8);
                rd(cand[i] + 0x848, &ph, 8);
                mlog("chop: cand %s=%p +4c8=%p +4d0=%p +10c8=%p +528=%llu +848=%llu\n",
                     names[i], (void *)cand[i], (void *)c0, (void *)d0, (void *)ctl,
                     (unsigned long long)(mode & 0xffffffff),
                     (unsigned long long)(ph & 0xffffffff));
            }
        }
        if (g_chop_editor) {
            uintptr_t es = 0, sel = 0, ctl = 0, ph = 0;
            rd(g_chop_editor + 0x4c8, &es, 8);
            rd(g_chop_editor + 0x4d0, &sel, 8);
            rd(g_chop_editor + 0x10c8, &ctl, 8);
            rd(g_chop_editor + 0x848, &ph, 8);
            mlog("chop: editor +0x4c8=%p +0x4d0=%p +0x10c8=%p +0x848=%llu\n",
                 (void *)es, (void *)sel, (void *)ctl, (unsigned long long)(ph & 0xffffffff));
        }
        return;
    }
    if (!strcmp(cmd, "rp")) {
        if (g_screen)
            FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(g_screen, 0, 0);
        mlog("chop: repaint issued\n");
        return;
    }
    if (!strncmp(cmd, "ww ", 3)) {
        unsigned long addr = 0, val = 0;
        if (sscanf(cmd + 3, "%lx %lx", &addr, &val) != 2 || !addr)
            return;
        uintptr_t old = 0;
        rd((uintptr_t)addr, &old, 8);
        int ok = wr((uintptr_t)addr, &val, 8);
        mlog("chop: ww %p %llu -> %llu ok=%d\n", (void *)addr,
             (unsigned long long)old, (unsigned long long)val, ok);
        return;
    }
    if (!strncmp(cmd, "findq ", 6)) {
        unsigned long long want = 0;
        if (sscanf(cmd + 6, "%llx", &want) != 1)
            return;
        FILE *f = fopen("/proc/self/maps", "r");
        if (!f)
            return;
        size_t chunk = 8u << 20;
        unsigned char *b = malloc(chunk);
        char line[512];
        int hits = 0;
        while (b && fgets(line, sizeof line, f) && hits < 16) {
            unsigned long lo = 0, hi = 0;
            char perms[8] = {0};
            if (sscanf(line, "%lx-%lx %7s", &lo, &hi, perms) != 3 || perms[1] != 'w')
                continue;
            for (unsigned long a = lo; a < hi && hits < 16; a += chunk) {
                size_t n = (hi - a) < chunk ? (size_t)(hi - a) : chunk;
                if (!rd(a, b, n))
                    continue;
                for (size_t o = 0; o + 8 <= n && hits < 16; o += 8) {
                    unsigned long long v;
                    memcpy(&v, b + o, 8);
                    if (v == want) {
                        mlog("chop: findq %llx at %p\n", want, (void *)(a + o));
                        hits++;
                    }
                }
            }
        }
        if (b)
            free(b);
        fclose(f);
        mlog("chop: findq done %d\n", hits);
        return;
    }
    if (!strncmp(cmd, "findval ", 8)) {
        unsigned long want = 0;
        if (sscanf(cmd + 8, "%lx", &want) != 1)
            return;
        FILE *f = fopen("/proc/self/maps", "r");
        if (!f)
            return;
        size_t chunk = 8u << 20;
        unsigned char *b = malloc(chunk);
        char line[512];
        int hits = 0;
        while (b && fgets(line, sizeof line, f) && hits < 24) {
            unsigned long lo = 0, hi = 0;
            char perms[8] = {0};
            if (sscanf(line, "%lx-%lx %7s", &lo, &hi, perms) != 3 || perms[1] != 'w')
                continue;
            for (unsigned long a = lo; a < hi && hits < 24; a += chunk) {
                size_t n = (hi - a) < chunk ? (size_t)(hi - a) : chunk;
                if (!rd(a, b, n))
                    continue;
                for (size_t o = 0; o + 4 <= n && hits < 24; o += 4) {
                    unsigned v;
                    memcpy(&v, b + o, 4);
                    if (v == (unsigned)want) {
                        mlog("chop: findval 0x%lx at %p\n", want, (void *)(a + o));
                        hits++;
                    }
                }
            }
        }
        if (b)
            free(b);
        fclose(f);
        mlog("chop: findval done %d\n", hits);
        return;
    }

    if (!strcmp(cmd, "snap")) {
        chop_snapshot("manual");
        return;
    }
    if (!strcmp(cmd, "snaptrack")) {
        g_snap_n = 0;
        g_snap_track = 1;
        mlog("chop: snapshot tracking on\n");
        return;
    }
    if (!strcmp(cmd, "snapoff")) {
        g_snap_track = 0;
        mlog("chop: snapshot tracking off\n");
        return;
    }
    if (!strcmp(cmd, "alloc")) {
        g_malloc_n = 0;
        g_malloc_probe = 1;
        mlog("chop: malloc probe on\n");
        return;
    }
    if (!strcmp(cmd, "allocoff")) {
        g_malloc_probe = 0;
        mlog("chop: malloc probe off\n");
        return;
    }
    if (!strncmp(cmd, "add ", 4)) {
        unsigned want = (unsigned)strtoul(cmd + 4, 0, 10);
        if (!g_chop_editor || !sample || !orig_add_slice) {
            mlog("chop: add refused (no editor/sample)\n");
            return;
        }
        uintptr_t sel = 0;
        if (!rd(g_chop_editor + 0x4d0, &sel, 8) || !sel) {
            mlog("chop: add refused (no selection)\n");
            return;
        }
        uintptr_t pair[2];
        pair[0] = sample;
        pair[1] = sel;
        mlog("chop: add frame=%u before=%u\n", want, chop_slice_count(sample));
        orig_add_slice((void *)g_chop_editor, pair, want);
        chop_dump("after-add", sample);
        return;
    }
    if (!strncmp(cmd, "addx ", 5)) {
        unsigned long ed = 0, frame = 0;
        if (sscanf(cmd + 5, "%lx %lu", &ed, &frame) != 2 || !sample || !orig_add_slice) {
            mlog("chop: addx refused\n");
            return;
        }
        uintptr_t sel = 0;
        rd(ed + 0x4d0, &sel, 8);
        uintptr_t pair[2] = {sample, sel};
        unsigned before = chop_slice_count(sample);
        mlog("chop: addx ed=%p sel=%p frame=%lu before=%u\n", (void *)ed, (void *)sel,
             frame, before);
        chop_add_guarded(ed, pair, frame);
        unsigned after = chop_slice_count(sample);
        mlog("chop: addx after=%u guard=%d %s\n", after, (int)g_chop_ok,
             !g_chop_ok ? "FAULTED" : (after == before + 1 ? "OK" : "UNEXPECTED"));
        chop_dump("after-addx", sample);
        return;
    }
    if (!strncmp(cmd, "rmallx ", 7)) {
        unsigned long ed = 0;
        if (sscanf(cmd + 7, "%lx", &ed) != 1 || !sample || !orig_rmall_cmd) {
            mlog("chop: rmallx refused\n");
            return;
        }
        uintptr_t ctl = 0, sel = 0;
        rd(ed + 0x10c8, &ctl, 8);
        rd(ed + 0x4d0, &sel, 8);
        uintptr_t pair[2] = {sample, sel};
        mlog("chop: rmallx ed=%p ctl=%p sel=%p before=%u\n", (void *)ed, (void *)ctl,
             (void *)sel, chop_slice_count(sample));
        chop_rmall_guarded(ctl, pair, ed);
        mlog("chop: rmallx after=%u guard=%d\n", chop_slice_count(sample), (int)g_chop_ok);
        chop_dump("after-rmall", sample);
        return;
    }
    if (!strncmp(cmd, "mk ", 3)) {
        unsigned long off = 0;
        unsigned n = 16;
        int got = sscanf(cmd + 3, "%lx %u", &off, &n);
        if (got < 1 || !off)
            return;
        if (n > 64)
            n = 64;
        uintptr_t addr = base + (uintptr_t)off;
        for (unsigned i = 0; i < n; ++i) {
            uintptr_t q = 0;
            if (!rd(addr + (uintptr_t)i * 8, &q, 8))
                break;
            mlog("chop:   m[0x%lx+%u] = 0x%llx\n", (unsigned long)off, i,
                 (unsigned long long)(q - base));
        }
        return;
    }
    if (!strncmp(cmd, "vl", 2)) {
        unsigned n = cmd[2] ? (unsigned)strtoul(cmd + 3, 0, 10) : 40;
        uintptr_t buf = 0, cont = 0, vt = 0;
        if (!sample || !rd(sample + 0x130, &buf, 8) || !buf)
            return;
        cont = buf + 0x10;
        if (!rd(cont, &vt, 8))
            return;
        mlog("chop: slicelist=%p vtable=%p off=0x%llx\n", (void *)cont, (void *)vt,
             (unsigned long long)(vt - base));
        for (unsigned i = 0; i < n; ++i) {
            uintptr_t q = 0;
            if (!rd(vt + (uintptr_t)i * 8, &q, 8))
                break;
            mlog("chop:   lv[%u] = 0x%llx\n", i, (unsigned long long)(q - base));
        }
        return;
    }
    if (!strcmp(cmd, "wvec")) {
        uintptr_t buf = 0, cont = 0, begin = 0;
        if (sample && rd(sample + 0x130, &buf, 8) && buf) {
            cont = buf + 0x10;
            rd(cont + 0x60, &begin, 8);
            mlog("chop: watch vector header %p and elements %p\n",
                 (void *)(cont + 0x68), (void *)begin);
            watch_arm(cont + 0x68, "slicevec");
            watch_arm(cont + 0x60, "slicevec0");
            if (begin)
                watch_arm(begin, "sliceelems");
        }
        return;
    }
    if (!strcmp(cmd, "samp")) {
        uintptr_t a = 0, b = 0, buf = 0;
        rd(sample + 0x170, &a, 8);
        rd(sample + 0x178, &b, 8);
        rd(sample + 0x130, &buf, 8);
        mlog("chop: sample=%p +0x170=%p +0x178=%p +0x130=%p\n", (void *)sample,
             (void *)a, (void *)b, (void *)buf);
        g_chop_ok = 0;
        if (sigsetjmp(g_cc_jmp, 1) == 0) {
            g_cc_recover = 1;
            unsigned r = FN(0x2943640, unsigned (*)(void *, unsigned))((void *)sample, 1);
            g_cc_recover = 0;
            g_chop_ok = 1;
            mlog("chop: isSampleValid=%u\n", r);
        } else {
            g_cc_recover = 0;
            mlog("chop: isSampleValid FAULTED\n");
        }
        return;
    }
    if (!strcmp(cmd, "count")) {
        chop_dump("count", sample);
        return;
    }
    if (!strncmp(cmd, "vt", 2)) {
        unsigned n = cmd[2] ? (unsigned)strtoul(cmd + 3, 0, 10) : 40;
        uintptr_t vt = 0;
        if (!sample || !rd(sample, &vt, 8))
            return;
        mlog("chop: sample=%p vtable=%p file_off=0x%llx entries=%u\n", (void *)sample,
             (void *)vt, (unsigned long long)(vt - base), n);
        for (unsigned i = 0; i < n; ++i) {
            uintptr_t q = 0;
            if (!rd(vt + (uintptr_t)i * 8, &q, 8))
                break;
            mlog("chop:   vt[%u] = 0x%llx\n", i, (unsigned long long)(q - base));
        }
        return;
    }
    (void)have;
}

static void *chop_probe_thread(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 200 * 1000 * 1000};
    for (;;) {
        nanosleep(&ts, 0);
        int fd = open("/run/minimod.chop", O_RDONLY);
        if (fd < 0)
            continue;
        char buf[64];
        ssize_t n = read(fd, buf, sizeof buf - 1);
        close(fd);
        unlink("/run/minimod.chop");
        if (n <= 0)
            continue;
        buf[n] = 0;
        char *nl = strchr(buf, '\n');
        if (nl)
            *nl = 0;
        memcpy(g_chop_cmd, buf, sizeof g_chop_cmd);
        mlog("chop: queued '%s'\n", g_chop_cmd);
    }
    return 0;
}
#endif

static void hpss_finalize(void)
{
    if (g_hpss_finalizing || !__atomic_load_n(&g_hpss_done, __ATOMIC_ACQUIRE))
        return;
    g_hpss_finalizing = 1;
    __atomic_store_n(&g_hpss_done, 0, __ATOMIC_RELEASE);
    struct hpss_job *job = g_hpss_job;
    g_hpss_job = NULL;
    if (!job) {
        g_hpss_busy = 0;
        g_hpss_finalizing = 0;
        return;
    }
    int effect = job->action != 0;
    if (job->rc != 0 ||
        (effect ? (!job->rendered || job->rendered_frames > UINT32_MAX)
                : (!job->harmonic || !job->percussive))) {
        sample_action_status(job->action, "processing failed");
        toast(effect ? "Effect: processing failed" : "HPSS: processing failed");
        goto out;
    }
    if (g_screen != job->screen || !sample_page_visible()) {
        sample_action_status(job->action, "cancelled (page changed)");
        toast(effect ? "Effect cancelled" : "HPSS cancelled");
        goto out;
    }
    {
        void *s = 0;
        uintptr_t st = 0, smp = 0;
        unsigned fr = 0, ch = 0, rt = 0;
        if (!hpss_read_sample(job->screen, &s, &st, &smp, &fr, &ch, &rt) ||
            st != job->state || smp != job->sample || fr != job->frames ||
            ch != job->ch || rt != job->rate) {
            sample_action_status(job->action, "cancelled (sample changed)");
            toast(effect ? "Effect cancelled" : "HPSS cancelled");
            goto out;
        }
    }
    {
        uintptr_t program = 0;
        int required = effect ? 1 : 2;
        if (!rd(job->state + 0x2310, &program, 8) || !program ||
            hpss_free_count((void *)program) < required) {
            sample_action_status(job->action, effect ? "need 1 empty pad" : "need 2 empty pads");
            toast(effect ? "Effect: need 1 pad" : "HPSS: need 2 pads");
            goto out;
        }
        if (effect) {
            int pad = -1;
            unsigned read_frames = job->frames;
            if (job->action == 4 && job->rendered_frames < read_frames)
                read_frames = (unsigned)job->rendered_frames;
            int ok = hpss_extract_ctx(job->screen, job->state, job->rendered, 0,
                                      read_frames, read_frames,
                                      (unsigned)job->rendered_frames,
                                      job->rendered_ch ? job->rendered_ch : job->ch,
                                      job->action + 1, &pad);
            hpss_restore_chop();
            int source_ok = bass_source_restore(job->source_path,
                                                job->source_data_offset,
                                                job->source_audio_backup,
                                                job->source_audio_bytes);
            if (!source_ok) {
                sample_action_status(job->action, "source restore failed");
                toast("Bass: source restore failed");
                goto out;
            }
            if (!ok) {
                sample_action_status(job->action, "output failed");
                toast("Effect: write failed");
                goto out;
            }
        } else {
            int first_pad = -1, second_pad = -1;
            int a = hpss_extract_ctx(job->screen, job->state, job->harmonic, 0,
                                     job->frames, job->frames, job->frames,
                                     job->ch, 0, &first_pad);
            if (!a) {
                hpss_restore_chop();
                hpss_set_status("HPSS: first output failed");
                toast("HPSS: write failed");
                goto out;
            }
            int b = hpss_extract_ctx(job->screen, job->state, job->percussive, 0,
                                     job->frames, job->frames, job->frames,
                                     job->ch, 1, &second_pad);
            hpss_restore_chop();
            if (!b) {
                /* Do not attempt a destructive rollback outside the native command. */
                hpss_set_status("HPSS: harmonic only (second failed)");
                toast("HPSS: 1 of 2");
                goto out;
            }
        }
    }
    if (effect) {
        sample_action_status(job->action, "copy created");
        toast(job->action == 1 ? "Reverb done" : job->action == 2 ? "Vinyl done" :
              job->action == 3 ? "Lo-Fi done" : "Bass C2 done");
    } else {
        hpss_set_status("HPSS: harmonic + percussive created");
        toast("HPSS done");
    }
    g_hpss_menu = 0;
    g_bass_edit = 0;
out:
    if (job->action == 4 && job->source_audio_backup &&
        !bass_source_restore(job->source_path, job->source_data_offset,
                             job->source_audio_backup, job->source_audio_bytes)) {
        sample_action_status(4, "source restore failed");
        toast("Bass: source restore failed");
    }
    hpss_free_job(job);
    g_hpss_busy = 0;
    g_hpss_menu = 0;
    bass_preview_set_active(0);
    g_hpss_data_down = 0;
    g_hpss_finalizing = 0;
    hpss_repaint();
}

static void hpss_open(void)
{
    if (!sample_page_visible() || g_hpss_busy)
        return;
    g_hpss_menu = 1;
    g_hpss_sel = 0;
    g_bass_edit = 0;
    bass_preview_set_active(0);
    g_hpss_data_down = 0;
    hpss_set_status("");
    hpss_repaint();
}

static void hpss_close(void)
{
    if (g_hpss_busy)
        return;
    g_hpss_menu = 0;
    g_bass_edit = 0;
    bass_preview_set_active(0);
    g_hpss_data_down = 0;
    hpss_repaint();
}

static void hpss_activate(void)
{
    if (g_hpss_busy)
        return;
    if (g_bass_edit) {
        fmbass_start();
        return;
    }
    if (g_hpss_sel == 0)
        hpss_start();
    else if (g_hpss_sel == 1)
        spectral_start();
    else if (g_hpss_sel >= 2 && g_hpss_sel <= 4)
        effect_start(g_hpss_sel - 1);
    else if (g_hpss_sel == 5) {
        g_bass_edit = 1;
        bass_preview_set_params(g_bass_knob[0], g_bass_knob[1], g_bass_knob[2]);
        bass_preview_set_active(1);
        hpss_set_status(g_bass_preview_available ? "" : "Live preview unavailable");
        hpss_repaint();
    }
    else
        hpss_close();
}

static void draw_colour(void *g, uint32_t v);
static void draw_rect(void *g, int x, int y, int w, int h);
static void draw_text(void *g, const char *s, int x, int y, int w, int h,
                      float size);

static void hpss_draw_menu(void *g)
{
    int x = 14, y = 8, w = 292, h = 224;
    draw_colour(g, 0xff101014);
    draw_rect(g, x, y, w, h);
    draw_colour(g, 0xffffb43c);
    draw_rect(g, x, y, w, 2);
    draw_rect(g, x, y + h - 2, w, 2);
    if (!g_bass_edit) {
        draw_colour(g, 0xffffffff);
        draw_text(g, "SAMPLE OPTIONS", x + 10, y + 6, w - 20, 22, 18.0f);
    }
    if (g_hpss_busy) {
        draw_text(g, "Processing...", x + 10, y + 70, w - 20, 24, 16.0f);
        if (g_hpss_status[0])
            draw_text(g, g_hpss_status, x + 10, y + 98, w - 20, 20, 12.0f);
    } else {
        if (g_bass_edit) {
            draw_colour(g, 0xffffb43c);
            draw_text(g, "BASS SYNTH", x + 12, y + 6, w - 24, 22, 18.0f);
            draw_colour(g, 0xffb8b8be);
            draw_text(g, "PADS C2-D#3  /  CHROMATIC", x + 12, y + 36,
                      w - 24, 16, 11.0f);

            const int col_x[] = {x + 16, x + 108, x + 200};
            const char *knob_labels[] = {"BRIGHT", "EXCITE", "FILTER"};
            for (int i = 0; i < 3; ++i) {
                draw_colour(g, 0xffffb43c);
                draw_text(g, i == 0 ? "K1" : i == 1 ? "K2" : "K3",
                          col_x[i], y + 72, 76, 16, 11.0f);
                draw_colour(g, 0xffffffff);
                draw_text(g, knob_labels[i], col_x[i], y + 90, 76, 18, 12.0f);
                draw_colour(g, 0xff36363d);
                draw_rect(g, col_x[i], y + 116, 76, 4);
                draw_colour(g, 0xffffb43c);
                draw_rect(g, col_x[i], y + 116,
                          (int)(76.0f * g_bass_knob[i] + 0.5f), 4);
                char value[8];
                snprintf(value, sizeof value, "%.2f", (double)g_bass_knob[i]);
                draw_colour(g, 0xffffffff);
                draw_text(g, value, col_x[i], y + 128, 76, 20, 14.0f);
            }

            draw_colour(g, 0xff48484f);
            draw_rect(g, x + 12, y + 164, w - 24, 1);
            if (g_hpss_status[0]) {
                draw_colour(g, 0xffe8e8ec);
                draw_text(g, g_hpss_status, x + 12, y + 168, w - 24, 14, 10.0f);
            }
            draw_colour(g, 0xffffb43c);
            draw_text(g, "PADS: PLAY   DATA: SAVE   SHIFT+DATA: BACK",
                      x + 10, y + 192, w - 20, 16, 10.0f);
            return;
        }
        static const char *titles[] = {
            "HPSS", "Spectral Chop", "Add Reverb", "Vinyl", "Lo-Fi (Decimator)",
            "Bass Synth", "Close"};
        for (int i = 0; i < 7; ++i) {
            int row_y = y + 32 + i * 24;
            if (g_hpss_sel == i) {
                draw_colour(g, 0xff3a2a10);
                draw_rect(g, x + 6, row_y, w - 12, 23);
                draw_colour(g, 0xffffffff);
            }
            draw_text(g, titles[i], x + 12, row_y + 2, w - 24, 18, 13.0f);
        }
        if (g_hpss_status[0])
            draw_text(g, g_hpss_status, x + 10, y + 200, w - 20, 14, 10.0f);
    }
    if (!g_hpss_busy && !g_hpss_status[0]) {
        draw_colour(g, 0xffb0b0b0);
        draw_text(g, "TURN: select    DATA: confirm", x + 10, y + h - 17,
                  w - 20, 14, 10.0f);
    }
}

static void buttons(void *event)
{

    unsigned code = event ? FIELD(event, 8, uint8_t) : 0;
    unsigned down = event ? FIELD(event, 9, uint8_t) : 0;
    if (event && rd((uintptr_t)event, g_hpss_btn, sizeof g_hpss_btn)) {
        g_hpss_btn_valid = 1;
        uintptr_t context = 0, surface = 0;
        memcpy(&context, g_hpss_btn, sizeof context);
        if (context && rd(context + 0x10, &surface, sizeof surface) && surface)
            __atomic_store_n(&g_data_signal, surface + 0x2da8, __ATOMIC_RELEASE);
    }
#if MINIMOD_DEBUG
    if (g_snap_track && g_snap_n < 24) {
        char tag[16];
        snprintf(tag, sizeof tag, "btn%02x%d", code, down ? 1 : 0);
        chop_snapshot(tag);
        g_snap_n++;
    }
#endif
    if (code == 0x6f) { /* DATA press/turn */
        static int consumed;
        if (__atomic_load_n(&g_spectral_active, __ATOMIC_ACQUIRE))
            consumed = 1;
        if (down && (g_hpss_menu || (g_shift && sample_page_visible())))
            consumed = 1;
        int visible = sample_page_visible();
        if (down && !g_hpss_data_down) {
            if (g_shift) {
                if (visible && !g_hpss_busy) {
                    if (g_hpss_menu)
                        hpss_close();
                    else
                        hpss_open();
                }
            } else if (g_hpss_menu) {
                hpss_activate();
            }
        }
        g_hpss_data_down = !!down;
        if (consumed || g_hpss_menu || (g_shift && visible)) {
            if (!down) consumed = 0;
            return;
        }
    }
    if (getenv("MINIMOD_VIS") && g_screen)
        mlog("minimod: vis code=0x%02x chopA=%d chopB=%d\n", code,
             FN(0x112fd50, int (*)(void *))((char *)g_screen + 0x15e0),
             FN(0x112fd50, int (*)(void *))((char *)g_screen + 0x1ee0));
    if (code == 0x31) { /* SHIFT */
        g_shift = down ? 1 : 0;
        if (!down) {
            g_trim_step = 0;      /* the cycle only lives while SHIFT is held */
            g_b3_down = 0;
            if (g_hpss_restore_pending) {
                g_hpss_restore_pending = 0;
                hpss_restore_chop();
            }
        }
        set_filter_text(down ? "AutoTrim" : "Filter");
        if (orig_buttons)
            orig_buttons(event);
        return;
    }
    if (g_hpss_menu || __atomic_load_n(&g_spectral_active, __ATOMIC_ACQUIRE))
        return;
    if (code == 0x0e && g_shift && !in_chop_mode()) { /* SHIFT + B3: 3 %, 6 %, 9 %, untrimmed */
        /* The button repeats while held, so act on the press edge only. */
        if (down && !g_b3_down) {
            /* Analysis is cheap now (block reads); it must stay on this thread
             * so the commit is queued for the very next repaint. */
            do_autotrim();
            g_trim_step = (g_trim_step + 1) % 4;
        }
        g_b3_down = down;
        return;
    }
    /* PLUS / MINUS move the 16 Levels row by an octave. The pitch hook only
     * runs for that screen, so bumping unconditionally is safe - and the old
     * guard (g_octave != 0) made the first press a no-op. */
    if (getenv("MINIMOD_BTNLOG")) {
        static int n;
        if (n < 24) {
            n++;
            unsigned char raw[16];
            if (rd((uintptr_t)event, raw, sizeof raw)) {
                mlog("minimod: btn ev="
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
                mlog("minimod: ev06 %02x %02x %02x %02x %02x %02x %02x %02x | "
                     "%02x %02x %02x %02x %02x %02x %02x %02x\n",
                     raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
                     raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14],
                     raw[15]);
        }
    }
    if (code == 0x28 && down) {
        g_levels = !g_levels;
        mlog("minimod: 16 LEVELS mode -> %d\n", g_levels);
    }
    if ((code == 0x36 || code == 0x37) && levels_active()) {
        int plus = (code == 0x36);
        if (down) {
            /* both keys at once = reset the row to its original pitch */
            if (plus ? g_minus_held : g_plus_held) {
                g_octave = 0;
                mlog("minimod: PLUS+MINUS -> pitch reset to original\n");
                toast("Pitch reset");
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
        mlog("minimod: button code=0x%02x down=%u\n", code, down);
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
    if (__atomic_load_n(&g_spectral_active, __ATOMIC_ACQUIRE))
        return;
    if (__atomic_load_n(&g_bass_preview_active, __ATOMIC_ACQUIRE) &&
        pad >= 0 && pad < 16) {
        bass_preview_note((unsigned)pad, vel);
        __atomic_add_fetch(&g_bass_preview_notes, 1, __ATOMIC_RELAXED);
        return;
    }
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
        mlog("minimod: note_on pad=%d vel=%.2f\n", pad, (double)vel);
    }
    if (orig_note_on)
        orig_note_on(input, src, pad, props, vel);
}

static void note_off(void *input, int src, int pad)
{
    if (g_hpss_menu && g_bass_edit && pad >= 0 && pad < 16)
        return;
    if (orig_note_off)
        orig_note_off(input, src, pad);
}

/* The firmware's audio writer accepts planar float channels, then converts
   them to S32 PCM. Mix the preview into private block storage so the live
   engine buffers are never changed. The normal device block is 128 frames. */
static void bass_audio_writer(void *self, void *input, size_t frames, int offset)
{
    static __thread float scratch[8][2048];
    float *channels[8];
    if (!__atomic_load_n(&g_bass_preview_active, __ATOMIC_ACQUIRE)) {
        bass_preview_mix(NULL, 0, 0, 44100); /* clear any ringing voices after close */
        if (orig_bass_audio_writer)
            orig_bass_audio_writer(self, input, frames, offset);
        return;
    }
    int count = FIELD(self, 0xd8, int32_t);
    if (count < 0)
        count = FIELD(self, 0xe4, int32_t);
    if (!orig_bass_audio_writer)
        return;
    if (count < 1 || count > 8 || frames > 2048 || !input) {
        orig_bass_audio_writer(self, input, frames, offset);
        return;
    }

    float **source = input;
    for (int i = 0; i < count; ++i) {
        if (!source[i]) {
            orig_bass_audio_writer(self, input, frames, offset);
            return;
        }
        memcpy(scratch[i], source[i], frames * sizeof(float));
        channels[i] = scratch[i];
    }
    float peak = bass_preview_mix(channels, frames, (unsigned)count, 44100);
    if (peak > 0.f) {
        __atomic_add_fetch(&g_bass_preview_blocks, 1, __ATOMIC_RELAXED);
        uint32_t previous_bits = __atomic_load_n(&g_bass_preview_peak_bits, __ATOMIC_RELAXED);
        float previous;
        memcpy(&previous, &previous_bits, sizeof previous);
        if (peak > previous)
            __atomic_store_n(&g_bass_preview_peak_bits, bass_float_bits(peak), __ATOMIC_RELAXED);
        orig_bass_audio_writer(self, channels, frames, offset);
    } else {
        orig_bass_audio_writer(self, input, frames, offset);
    }
}

static void *g_cs_self;

static void cs_event(void *self, int code, void *payload)
{
#if MINIMOD_LOG
    {
        static int n;
        if (n < 200) {
            unsigned char raw[16];
            memset(raw, 0, sizeof raw);
            if (payload && rd((uintptr_t)payload, raw, sizeof raw)) {
                n++;
                mlog("cs %02x | %02x%02x%02x%02x %02x%02x%02x%02x\n",
                     code & 0xff, raw[0], raw[1], raw[2], raw[3],
                     raw[4], raw[5], raw[6], raw[7]);
            }
        }
    }
#endif
    if (self)
        g_cs_self = self;
    if (debug_mode && g_dbg < 160) {
        g_dbg++;
        mlog("minimod: cs code=0x%02x\n", code & 0xff);
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
            mlog("minimod: obs property=%p v=%.4f\n", self, (double)v);
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
                mlog("minimod: reg x%2d=%p propish=%d c50=%.4f\n", 19 + i, (void *)r,
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
            mlog("minimod: knob param=%p slider=%p (%.2f%%)\n", self, (void *)sl,
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
            mlog("minimod: diag prop=%p v50=%s%.4f refs=%d v=%.4f\n", (void *)prop,
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
                    mlog("minimod: prop %p head=%p sentinel=%p\n", (void *)prop,
                         (void *)head, (void *)(prop + 8));
                    for (int i = 0; i < 8 && node && node != prop + 8; ++i) {
                        uintptr_t tgt = 0, next = 0;
                        if (!rd(node + 0x18, &tgt, 8) || !rd(node + 8, &next, 8))
                            break;
                        mlog("minimod:   node %p target=%p next=%p\n", (void *)node,
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
            mlog("minimod: learn? prop=%p c50=%s%.4f v=%.4f\n", (void *)prop,
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
                    mlog("minimod: learnt self=%p prop=%p v=%.4f\n", self,
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
            mlog("minimod: layer@knob %p 4d8=%u 530=%u 588=%u\n", (void *)lay, a, b, c);
        }
        uintptr_t vt = 0, val = 0, ap = 0;
        rd((uintptr_t)self, &vt, 8);
        rd((uintptr_t)self + T_VALUE, &val, 4);
        rd((uintptr_t)self + T_APPLIED, &ap, 4);
        logs++;
        mlog("minimod: trim self=%p vptr=%p(base+0x%lx) v=%.4f val=%.4f ap=%.4f lr=%p\n",
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
        mlog("minimod: bcast %p v=%.4f\n", self, (double)v);

    /* Record the property a *native* broadcast uses: that is the object the
     * page listens to. Our own broadcasts are flagged. */
    if (!g_own_call) {
        static int lrc;
        int nice = (v > 0.001f && v < 0.999f) && v != 0.5f;
        if (nice && lrc < 24) {
            lrc++;
            mlog("minimod: bcast lr=%p prop=%p v=%.4f\n",
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
            mlog("minimod: native bcast property=%p v=%.4f\n", self, (double)v);
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
    mlog("minimod: %d property candidates from bindings\n", g_ncand);
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
            mlog("minimod: owner of param=%p is prop=%p\\n", (void *)param,
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
                mlog("minimod: controls prop +0x%lx references %p\n", (unsigned long)o,
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
            mlog("minimod: slider +0x%lx %.2f -> %.2f\n", o, (double)v, (double)to);
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
    mlog("minimod: instance %p sliders %p/%p -> %.2f/%.2f (was %.2f/%.2f)\n",
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
                mlog("minimod: page base %p vptr=%p (vt+0x%lx)\n", (void *)b,
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
        mlog("minimod: trim page %p (score %.2f)\n", (void *)best, (double)bestscore);
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
                mlog("minimod: node %p owner=%p next=%p back=%p\n", (void *)node,
                     (void *)owner, (void *)next, (void *)back);
                if (owner && mapped((void *)owner)) {
                    uintptr_t head = 0;
                    rd(owner + 0x10, &head, 8);
                    float c50 = 0;
                    int ok50 = rd(owner + 0x50, &c50, 4) && c50 >= -1.0f && c50 <= 1.0f;
                    mlog("minimod:   owner+0x10=%p c50=%.4f (%d)\n", (void *)head,
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
static unsigned char g_cc_k3_ev[32];
static volatile int g_cc_k3_valid;
static unsigned g_cc_raw[2];
static float g_cc_range[2] = {127.0f, 127.0f};
static int g_cc_seen[2];
static int g_in_cc_replay;

static void cc_capture(void *ev, unsigned raw, float lo, float range)
{
    if (!g_in_cc_replay && ev && g_hpss_menu && g_bass_edit && !g_hpss_busy) {
        unsigned cc = ((unsigned char *)ev)[8];
        int knob = cc == 0x10 ? 0 : cc == 0x11 ? 1 : cc == 0x12 ? 2 : -1;
        if (knob >= 0) {
            float scale = range > 1.0f ? range : 127.0f;
            g_bass_knob[knob] = fminf(1.f, fmaxf(0.f, raw / scale));
            bass_preview_set_params(g_bass_knob[0], g_bass_knob[1], g_bass_knob[2]);
            hpss_repaint();
            return;
        }
    }
    /* Keep the stock decoder/transport intact. Only DATA reaches the native
     * integer signal while our modal menu owns controls; knobs must not edit
     * the sample behind it. Replayed internal commands are not user input. */
    if (!g_in_cc_replay && ev &&
        (g_hpss_menu || __atomic_load_n(&g_spectral_active, __ATOMIC_ACQUIRE)) &&
        ((unsigned char *)ev)[8] != 0x64)
        return;
#if MINIMOD_DEBUG
    if (g_snap_track && !g_in_cc_replay && ev && g_snap_n < 24) {
        char tag[24];
        snprintf(tag, sizeof tag, "cc%02x=%u", ((unsigned char *)ev)[8], raw);
        chop_snapshot(tag);
        g_snap_n++;
    }
#endif
    if (!g_in_cc_replay && ev) {
        unsigned char cc = ((unsigned char *)ev)[8];
        static int allc;
        if (allc < 16) {
            allc++;
            mlog("minimod: cc_in cc=0x%x raw=%u range=%.1f ev0=%p\n", cc, raw,
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
                mlog("minimod: captured CC event (cc=0x%x raw=%u range=%.1f)\n", cc,
                     raw, (double)range);
            }
            g_cc_raw[idx] = raw;
            if (range > 1.0f)
                g_cc_range[idx] = range;
            g_cc_seen[idx] = 1;
        }
        if (cc == 0x12 && !g_cc_k3_valid) {
            memcpy(g_cc_k3_ev, ev, sizeof g_cc_k3_ev);
            g_cc_k3_valid = 1;
        }
    }
    if (orig_cc)
        orig_cc(ev, raw, lo, range);
}

static void cc_replay(unsigned cc, unsigned raw)
{
    unsigned char ev[32];
    if (!g_cc_ev_valid) {
        mlog("Spectral: no captured CC event, cannot replay\n");
        return;
    }
    memcpy(ev, g_cc_ev, sizeof ev);
    ev[8] = (unsigned char)cc;
    g_in_cc_replay = 1;
    if (orig_cc)
        orig_cc(ev, raw, 0.0f, 127.0f);
    g_in_cc_replay = 0;
}

static void cc_replay_k3(unsigned raw)
{
    if (!g_cc_k3_valid)
        return;
    unsigned char ev[32];
    memcpy(ev, g_cc_k3_ev, sizeof ev);
    ev[8] = 0x12;
    g_in_cc_replay = 1;
    if (orig_cc)
        orig_cc(ev, raw, 0.0f, 127.0f);
    g_in_cc_replay = 0;
    mlog("Spectral: replayed K3=%u (valid=%d)\n", raw, g_cc_k3_valid);
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
    if (sigsetjmp(g_cc_jmp, 1) == 0) {
        g_cc_recover = 1;
        FN(0x159f230, void (*)(void *, unsigned, float, float))(ev, raw, 0.0f, 127.0f);
        g_cc_recover = 0;
    } else {
        mlog("minimod: replay faulted - recovered\n");
    }
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
    mlog("minimod: heap snapshot '%s': %d pages\n", tag, g_nhp);
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
            mlog("minimod: changed page %p (%s)\n", (void *)g_hp[i].addr, tag);
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
        mlog("minimod: knob triple %p score=%d\n", (void *)best, bestscore);
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
        mlog("minimod: poke fire %d v=%.3f\n", which, (double)g_poke_value);
        uintptr_t pp = prop_of_param((uintptr_t)g_trim_param0);
        if (!pp)
            pp = prop_of_param((uintptr_t)g_trim_param1);
        if (!pp) {
            uintptr_t kp = find_knob_props();
            if (kp)
                pp = kp;
        }
        mlog("minimod: poke prop=%p\n", (void *)pp);
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
                    mlog("minimod: poke %d target=%p slot=%p v=%.4f\n", which, (void *)t,
                         (void *)slot, (double)g_poke_value);
                    ((void (*)(float, void *))slot)(g_poke_value, (void *)t);
                    mlog("minimod: poke done\n");
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
    if (getenv("MINIMOD_NODEPROBE"))
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
    mlog("minimod: props %p/%p (cur %.4f/%.4f, seen %d)\n", (void *)pr0,
         (void *)pr1, (double)cur0, (double)cur1, g_nbc);

    /* Exact pair from the knob properties, when we can find them. */
    {
        uintptr_t kp = find_knob_props();
        if (kp) {
            uintptr_t sp = param_from_prop(kp), ep = param_from_prop(kp + 0x58);
            mlog("minimod: params from props %p/%p (had %p/%p)\n", (void *)sp, (void *)ep,
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
    if (getenv("MINIMOD_CCINJECT") && !g_in_inject) {
        g_cc_step = 0;
        g_pending_cc = 1;
    }
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
                mlog("minimod: knob property triple at %p\n", (void *)kp);
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
                mlog("minimod: control base %p rejected\n", (void *)base);
                base = 0;
            } else {
                obj = ctl_obj_for(base);
            }
        }
        mlog("minimod: cc base=%p obj=%p\n", (void *)base, (void *)obj);
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
                        mlog("minimod: P%p listener %p vt=%p slot28=%p\n", (void *)P,
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
            /* Default: one control per paint (safe, but the page does not
             * update). MINIMOD_ONESHOT delivers both in one turn, which does
             * update the model - and leaves the app faulting afterwards. */
            if (getenv("MINIMOD_ONESHOT")) {
                if (g_cc_ev_valid) {
                    cc_replay(0x11, r1);
                    cc_replay(0x10, r0);
                } else {
                    cc_inject(obj, 0x11, r1);
                    cc_inject(obj, 0x10, r0);
                }
                mlog("minimod: one-shot replay done\n");
                return;
            }
            if (g_cc_step == 0) {
                g_cc_step = 1;
                g_cc_obj = obj;
                g_cc_r0 = r0;
                g_cc_r1 = r1;
                if (g_cc_ev_valid)
                    cc_replay(0x11, r1);
                else
                    cc_inject(obj, 0x11, r1);
                g_pending_cc = 1; /* come back for the second one */
                mlog("minimod: step 1 done\n");
                return;
            }
            if (g_cc_step == 1) {
                g_cc_step = 2;
                if (g_cc_ev_valid)
                    cc_replay(0x10, g_cc_r0);
                else
                    cc_inject(g_cc_obj, 0x10, g_cc_r0);
                mlog("minimod: step 2 done\n");
                return;
            }
            mlog("minimod: injected CC 0x10=%u 0x11=%u\n", r0, r1);
            {
                float c0 = -9, c1 = -9;
                if (pr0)
                    rd(pr0 + 0x50, &c0, 4);
                if (pr1)
                    rd(pr1 + 0x50, &c1, 4);
                mlog("minimod: app stored %.4f/%.4f (want %.4f/%.4f)\n", (double)c0,
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
                    mlog("minimod: re-selected pad %d to refresh the page\n", pd);
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
                mlog("minimod: repaint root=%p (page %p)\n", (void *)root, g_screen);
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
        mlog("minimod: retry %d (p0=%.3f p1=%.3f)\n", attempt, (double)c0, (double)c1);
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
        mlog("minimod: layer %p <- %u..%u frames\n", (void *)g_trim_layer, fa, fb);
    }

    float a0 = v0, a1 = v1;
    wr((uintptr_t)p0 + T_APPLIED, &a0, 4);
    wr((uintptr_t)p1 + T_APPLIED, &a1, 4);

    float check = 0, check2 = 0;
    rd((uintptr_t)p0 + T_VALUE, &check, 4);
    rd((uintptr_t)p1 + T_VALUE, &check2, 4);
    /* Also write the property caches: the broadcaster stores the value there
     * before notifying, so anything that polls the property sees the new trim. */
    /* NOTE: do NOT store prop+0x50 directly. Property_Broadcast(value, prop)
     * early-outs when the stored value already equals the new one, so writing
     * the slot first suppresses the listener notification (which is what the
     * page needs in order to update and redraw). The broadcast below does the
     * store itself. */
    /* Move the app's own slider widgets too: the page draws the Start/End bars
     * from them at paint time. */
    for (int i = 0; i < g_nps; ++i) {
        float pct = -1.0f;
        if (g_ps[i].param == (uintptr_t)p0)
            pct = v0 * 100.0f;
        else if (g_ps[i].param == (uintptr_t)p1)
            pct = v1 * 100.0f;
        if (pct >= 0.0f && g_ps[i].slider) {
            if (wr(g_ps[i].slider + 0x1b4, &pct, 4))
                mlog("minimod: slider %p -> %.2f%%\n", (void *)g_ps[i].slider,
                     (double)pct);
        }
    }
    g_mark_v0 = v0;
    g_mark_v1 = v1;
    mlog("minimod: commit core done p0=%.4f p1=%.4f (want %.4f/%.4f)\n",
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
        mlog("minimod: re-pressed %p to refresh the page\n", (void *)g_last_press_obj);
    }
    /* Repaint the page itself: with the property values written above, this is
     * what makes the app's own Start/End edge follow the trim. */
    if (g_livepage) {
        uintptr_t vt = 0;
        if (rd((uintptr_t)g_livepage, &vt, 8) && vt >= base && vt <= base + 0x12000000ULL) {
            mlog("minimod: page vt=%#lx file=%#lx\n", (unsigned long)vt,
                 (unsigned long)(vt - base));
            for (int i = 20; i < 32; ++i) {
                uintptr_t fn = 0;
                if (rd(vt + i * 8, &fn, 8) && fn >= base && fn <= base + 0x12000000ULL)
                    mlog("minimod:   slot %d -> %#lx\n", i, (unsigned long)(fn - base));
            }
        }
    }
    if (g_livepage) g_page = g_livepage;
    uintptr_t pvt = 0;
    if (g_page && rd((uintptr_t)g_page, &pvt, 8) &&
        pvt >= base && pvt <= base + 0x12000000ULL) {
        FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
            g_page, 0, FIELD(g_page, 0x28, uint64_t));
        mlog("minimod: repainted page %p\n", g_page);
    }
    /* Dry run: report the surface we would drive the controls with. */
    if (g_surface) {
        uintptr_t svt = 0;
        int ok = rd((uintptr_t)g_surface, &svt, 8) && svt >= base && svt <= base + 0x12000000ULL;
        mlog("minimod: surface=%p vt=%p valid=%d raw0=%d raw1=%d\n",
             g_surface, (void *)svt, ok, (int)(v0 * 127.0f + 0.5f),
             (int)(v1 * 127.0f + 0.5f));
    }
    /* Drive the app's own control path exactly like a knob turn:
     * FUN_01a68020(surface, 0, controlId, value, mode) - the first argument is
     * the surface (it dispatches through surface->vtable+0x160). */
    /* The control-path call is experimental: it executes cleanly but does not
     * move the app's display, so it stays behind an env flag. */
    if (getenv("MINIMOD_CTL")) {
        g_surface2 = g_surface;
        orig_setctl2 = orig_setctl;
        g_pending_r0 = (int)(v0 * 127.0f + 0.5f);
        g_pending_r1 = (int)(v1 * 127.0f + 0.5f);
        g_pending_v0 = v0 * g_trim_frames;
        g_pending_v1 = v1 * g_trim_frames;
        g_pending_ctl = 1;
        mlog("minimod: armed paint-time ctl %d/%d\n", g_pending_r0, g_pending_r1);
    }
    /* Notify the property's listeners: the page syncs its widget from the
     * property when it is told the value changed. */
    if (orig_bcast) {
        g_own_call = 1;
        if (pr0)
            orig_bcast((void *)pr0, v0);
        if (pr1)
            orig_bcast((void *)pr1, v1);
        g_own_call = 0;
        mlog("minimod: notified props %p/%p\n", (void *)pr0, (void *)pr1);
    }
    /* Also ask the screen to repaint with a clean (0,0) origin. */
    if (g_screen) {
        FN(0x1149880, void (*)(void *, uint64_t, uint64_t))((void *)g_screen, 0, 0);
        mlog("minimod: repainted screen %p\n", (void *)g_screen);
    }
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
/* The Trim page is its own class (found by matching its cached percentages);
 * it is not the plain SampleModeScreen. */
#define A_TRIM_PAGE_VT 0x6e31ee8

static void (*orig_screen_paint)(void *, void *);

/* One-second toast after a trim: text plus a monotonic deadline, drawn by the
 * paint hook so it costs nothing when idle. */
static volatile long long g_toast_until;
static char g_toast_text[48];

static long long now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void *toast_expiry(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 1, .tv_nsec = 120 * 1000 * 1000};
    nanosleep(&ts, 0);
    /* the app only repaints on change, so the last frame would keep the box up */
    mlog("minimod: toast expiry comp=%p repaint=%p\n", g_paint_comp, orig_repaint);
    if (orig_repaint && g_paint_comp) {
        /* the entry takes two packed pairs: (x,y) and (w,h) */
        orig_repaint(g_paint_comp, 0, ((uint64_t)2000 << 32) | (uint64_t)2000);
        mlog("minimod: toast expiry repainted the screen\n");
    }
    return 0;
}

static void toast(const char *text)
{
    size_t n = strlen(text);
    if (n >= sizeof g_toast_text)
        n = sizeof g_toast_text - 1;
    memcpy(g_toast_text, text, n);
    g_toast_text[n] = 0;
    g_toast_until = now_ms() + 1000;
    {
        void *comp = g_paint_comp ? g_paint_comp : g_screen;
        if (orig_repaint && comp)
            orig_repaint(comp, 0, ((uint64_t)2000 << 32) | (uint64_t)2000);
    }
    pthread_t th;
    int cr = pthread_create(&th, 0, toast_expiry, 0);
    if (cr == 0)
        pthread_detach(th);
    else
        mlog("minimod: toast thread failed %d\n", cr);
}

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
        mlog("minimod: slider +0x%lx\n", o);
        for (long k = 0x160; k < 0x230; k += 4) {
            float f = 0;
            if (!rd(s + k, &f, 4))
                break;
            if (f > -0.01f && f < 100.5f && f != 0.0f)
                mlog("minimod:   +0x%lx = %.4f\n", k, (double)f);
        }
    }
    mlog("minimod: dump done, %u sliders\n", n);
}

static void screen_paint(void *p, void *g)
{
    static int logged;
    g_paint_comp = p;
    g_paint_gfx = g;
    if (!logged && p) {
        uintptr_t vt = 0;
        if (rd((uintptr_t)p, &vt, 8) && vt >= base && vt <= base + 0x12000000ULL) {
            logged = 1;
            mlog("minimod: paint comp=%p vt=%#lx file=%#lx\n",
                 p, (unsigned long)vt, (unsigned long)(vt - base));
            for (int i = 24; i < 32; ++i) {
                uintptr_t fn = 0;
                if (rd(vt + i * 8, &fn, 8) && fn >= base && fn <= base + 0x12000000ULL)
                    mlog("minimod:   vt[%d] -> %#lx\n", i, (unsigned long)(fn - base));
            }
        }
    }
    if (orig_screen_paint)
        orig_screen_paint(p, g);
    /* The knob replay runs here, on the UI thread at a safe point, instead of
     * inside the button dispatch where it corrupted the app's state. */
    if (g_sdump) {
        g_sdump = 0;
        dump_sliders(p);
    }
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
        mlog("minimod: screen=%p\n", p);
    }
#if MINIMOD_DEBUG
    chop_probe_run(); /* model probes must also run on the UI thread */
#endif
    spectral_tick();
    if (__atomic_load_n(&g_hpss_done, __ATOMIC_ACQUIRE))
        hpss_finalize();
    if (g_hpss_restore_pending && !g_shift && sample_page_visible()) {
        g_hpss_restore_pending = 0;
        hpss_restore_chop();
    }
    {
        static int dumped;
        if (!dumped && getenv("MINIMOD_DUMP")) {
            dumped = 1;
            dump_sliders(p);
        }
    }
    /* Diff the painting screen between paints: whatever changes when a knob
     * turns is exactly what the page stores for its markers. */
    if (getenv("MINIMOD_SNAPDIFF")) {
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
                mlog("minimod: snapdiff baseline %p (%zu bytes)\n", p, got);
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
                mlog("minimod: diff +0x%zx %.4f -> %.4f\n", i, (double)a, (double)b);
                memcpy(prev + i, cur + i, 4);
                shown++;
            }
        }
    }
    /* Trim edges of the last autotrim, drawn where the page's own markers would
     * be: the page itself only re-reads its trim when the pad is re-selected, so
     * without this the result of SHIFT+B3 would be invisible. */
    if (g_toast_until > now_ms() && g_toast_text[0]) {
        /* Cover the Start/End/Loop bar. Near-black fill, an amber rule top and
         * bottom (the colour the page already uses for the waveform) and white
         * 18 pt text, so it reads at a glance without looking like a dialog. */
        int x = 0, y = 194, w = 320, h = 46;
        mlog("minimod: toast draw '%s' comp=%p vt=%#lx\n", g_toast_text, p,
             (unsigned long)(FIELD(p, 0, uintptr_t) - base));
        draw_colour(g, 0xff15151a);
        draw_rect(g, x, y, w, h);
        draw_colour(g, 0xffffb43c);
        draw_rect(g, x, y, w, 2);
        draw_rect(g, x, y + h - 2, w, 2);
        draw_colour(g, 0xffffffff);
        draw_text(g, g_toast_text, x + 4, y + 6, w - 8, h - 12, 18.0f);
    }
    if (g_mark_v0 >= 0.0f && g_mark_v1 >= 0.0f && !getenv("MINIMOD_NOOVERLAY")) {
        int x0 = 10 + (int)(g_mark_v0 * 295.0f + 0.5f);
        int x1 = 10 + (int)(g_mark_v1 * 295.0f + 0.5f);
        /* The page fills the trimmed-out parts with the waveform background
         * (27,27,30); draw the same so the picture matches a manual trim. */
        draw_colour(g, 0xff1b1b1e);
        if (x0 > 10)
            draw_rect(g, 10, 88, (uint64_t)(x0 - 10), 98);
        if (x1 < 305)
            draw_rect(g, (uint64_t)x1, 88, (uint64_t)(305 - x1), 98);
        draw_colour(g, 0xffffffff);
        draw_rect(g, (uint64_t)x0, 88, 1, 98);
        draw_rect(g, (uint64_t)x1, 88, 1, 98);
        draw_rect(g, (uint64_t)x0, 88, 3, 1);
        draw_rect(g, (uint64_t)x1, 88, 3, 1);
    }
    if (g_hpss_menu) {
        hpss_draw_menu(g);
        return;
    }
    if (!g_shift || in_chop_mode())
        return;
    /* third soft-key cell of the Sample page */
    draw_colour(g, 0xff000000);
    draw_rect(g, 213, 0, 107, 38);
    draw_colour(g, 0xffffffff);
    draw_text(g, "AutoTrim", 215, 5, 103, 28, 18.0f);
}

static int install_screen_paint(void)
{
    uintptr_t vtaddr = base + A_SCREEN_VT;
    if (!mapped((void *)vtaddr))
        return 0;
    uintptr_t *vt = (uintptr_t *)vtaddr;
    orig_screen_paint = (void (*)(void *, void *))vt[27];
    mlog("minimod: paint fn = %p (file %#lx)\n",
         (void *)orig_screen_paint, (unsigned long)((uintptr_t)orig_screen_paint - base));
    if (!orig_screen_paint)
        return 0;
    uintptr_t page = vtaddr & ~(uintptr_t)0xfff;
    if (mprotect((void *)page, 0x1000, PROT_READ | PROT_WRITE) != 0) {
        mlog("minimod: screen vtable mprotect failed: %s\n", strerror(errno));
        return 0;
    }
    vt[27] = (uintptr_t)screen_paint;
    mprotect((void *)page, 0x1000, PROT_READ);
    mlog("minimod: screen paint hooked (stock %p)\n", (void *)orig_screen_paint);
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
                    mlog("minimod: filter string at %p\n", (void *)str);
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
        mlog("minimod: label '%s' dest=%p\n", text, dest);
    }
    if (text && !strcmp(text, "Filter"))
        g_filter_dest = dest;
    if (g_shift && text && !strcmp(text, "Filter"))
        text = "AutoTrim";
    if (orig_str_assign)
        orig_str_assign(dest, text);
}



/* ------------------------------------------------------------ MIDI (feature 2)
 * Keep the firmware's MIDI input, note-on/off matching, sustain and audio
 * routing intact. The SampQOL reference confirms the native pitch setter takes
 * the event property block at +0x14 and a semitone delta as argument four. */
static void midi_note_event(void *app, void *context, void *event,
                            uintptr_t source)
{
    if (event) {
        uint32_t type = 0;
        float velocity = 0.0f;
        uint8_t note = 0xff;
        memcpy(&type, (char *)event + 8, sizeof type);
        memcpy(&velocity, (char *)event + 0x10, sizeof velocity);
        memcpy(&note, (char *)event + 0x30, sizeof note);
        int32_t source_id = (int32_t)(uint32_t)source;
        if (levels_active() &&
            (source_id <= 0x7fffffef || source_id == 0x7ffffff6) &&
            (type & 0xffu) == 3 && velocity > 0.0f && note <= 127 &&
            orig_pitch_off) {
            /* MIDI 60 (C3) is the stock keyboard root. Use the pitch setter's
             * trampoline so PLUS/MINUS's 16 Levels wrapper cannot alter this
             * explicit keyboard interval. */
            orig_pitch_off((char *)event + 0x14, 0, 0, (int)note - 60);
            g_midi_chromatic_note = note;
            g_midi_chromatic_delta = (int)note - 60;
            __atomic_add_fetch(&g_midi_chromatic_count, 1, __ATOMIC_RELAXED);
        }
    }
    if (orig_midi_note_event)
        orig_midi_note_event(app, context, event, source);
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
    int pad = g_last_pad >= 0 && g_last_pad < 128 ? g_last_pad : 0;
    uintptr_t array = 0, end = 0, state = 0, layers = 0, sample = 0;
    if (!g_surface || !g_control_context || !orig_setctl ||
        !rd((uintptr_t)g_surface + 0x2808, &array, 8) ||
        !rd((uintptr_t)g_surface + 0x2810, &end, 8) ||
        !array || end < array || (end-array)/8 <= (unsigned)pad ||
        !rd(array + pad*8, &state, 8) || !state ||
        !rd(state + 0x22e8, &layers, 8) || !layers ||
        !rd(layers + 0x4a8, &sample, 8) || !sample) {
        mlog("minimod: native trim: no sample for pad %d\n", pad);
        return;
    }
    unsigned frames = frames_of_sample(sample), fr=0, rate=0, ch=0, off=0, bytes=0;
    unsigned first=0, last=0, format=0;
    char path[512] = {0};
    if (frames) {
        if (path_from_object(sample, frames, path, sizeof path))
            mlog("minimod: path from the sample object -> %s\n", path);
        else if (wav_path_of(sample, frames, path, sizeof path))
            ;
        else if (find_wav_by_frames(frames, path, sizeof path))
            mlog("minimod: recorded sample (no path yet) -> %s\n", path);
    }
    if (!frames || !path[0] ||
        !wav_info(path, &fr, &rate, &ch, &off, &bytes, &format) || fr != frames) {
        return;
    }
    if (g_trim_step >= 3) {
        /* fourth press: remove the trim entirely */
        first = 0;
        last = frames ? frames - 1 : 0;
    } else {
        g_trim_ratio = g_trim_step == 0 ? 0.03
                     : g_trim_step == 1 ? 0.06 : 0.09;
        int ok = wav_edges(path, fr, off, ch, bytes, format, &first, &last);
        g_trim_ratio = 0.0;
        if (!ok)
            return;
    }
    /* Arm the toast first: the trim's own repaint is what paints the page, so
     * the box has to be set before the dispatch or it only appears on the next
     * unrelated repaint (previously SHIFT release). */
    switch (g_trim_step) {
    case 0: toast("Trim 3% of peak"); break;
    case 1: toast("Trim 6% of peak"); break;
    case 2: toast("Trim 9% of peak"); break;
    default: toast("Trim cleared"); break;
    }
    orig_setctl(g_control_context, g_surface, pad, 0x462, 0.0f);
    orig_setctl(g_control_context, g_surface, pad, 0x466, (float)(last+1));
    orig_setctl(g_control_context, g_surface, pad, 0x462, (float)first);
    {
        void *comp = g_paint_comp ? g_paint_comp : g_screen;
        if (orig_repaint && comp)
            orig_repaint(comp, 0, ((uint64_t)2000 << 32) | (uint64_t)2000);
    }
    mlog("minimod: native trim pad=%d path='%s' frames=%u bounds=%u..%u\n",
         pad, path, frames, first, last+1);
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

/* Probe: /run/minimod.slider holds "N,V" - write percent V into the N-th
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
        int fd = open("/run/minimod.slider", O_RDONLY);
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
        mlog("minimod: probe slider %d @%p = %.2f\n", idx, (void *)hit, (double)val);
        FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
            g_screen, 0, FIELD(g_screen, 0x28, uint64_t));
    }
    return 0;
}

/* Probe: /run/minimod.heap holds "snap" or "diff" for the heap fingerprint. */
static void *heap_probe_thread(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 400 * 1000 * 1000};
    for (;;) {
        nanosleep(&ts, 0);
        apply_pending_ctl();
        int fd = open("/run/minimod.heap", O_RDONLY);
        if (fd < 0)
            continue;
        char buf[32];
        ssize_t n = read(fd, buf, sizeof buf - 1);
        close(fd);
        unlink("/run/minimod.heap");
        if (n <= 0)
            continue;
        buf[n] = 0;
        if (!strncmp(buf, "snap", 4))
            heap_snapshot("mark");
        else if (!strncmp(buf, "diff", 4))
            heap_diff("after knob");
        else if (!strncmp(buf, "find ", 5))
            find_float_pair((float)atof(buf + 5));
        else if (!strncmp(buf, "model", 5)) {
            /* Resolve the pad-state container from the trim parameter and show
             * the fields that track the Start/End knobs. */
            uintptr_t pr = g_npm > 0 ? g_pm[0].param : (uintptr_t)g_trim_param0;
            uintptr_t c = 0;
            const char *sp = strchr(buf, ' ');
            if (sp) {
                float v = (float)atof(sp + 1);
                if (pr && rd(pr + 0x90, &c, 8) && c) {
                    int ok = wr(c + 0x7ea0, &v, 4);
                    mlog("minimod: model c=%p +0x7ea0 <- %.4f ok=%d\n", (void *)c,
                         (double)v, ok);
                }
            }
            if (pr && rd(pr + 0x90, &c, 8) && c) {
                mlog("minimod: model param used = %p\n", (void *)pr);
                for (unsigned off = 0x7000; off <= 0x9000; off += 4) {
                    float f = 0;
                    if (rd(c + off, &f, 4) && f > -0.01f && f < 1.01f)
                        mlog("minimod:   c+%#x = %.4f\n", off, (double)f);
                }
            }
        } else if (!strncmp(buf, "set ", 4)) {
            uintptr_t a = (uintptr_t)strtoull(buf + 4, 0, 16);
            const char *sp = strchr(buf + 4, ' ');
            float v = sp ? (float)atof(sp + 1) : 0.0f;
            int ok = wr(a, &v, 4);
            mlog("minimod: set %p = %.4f ok=%d\n", (void *)a, (double)v, ok);
        } else if (!strncmp(buf, "snapb", 5))
            byte_snap();
        else if (!strncmp(buf, "diffb", 5))
            byte_diff();
        else if (!strncmp(buf, "who ", 4))
            who_points_at((uintptr_t)strtoull(buf + 4, 0, 16));
        else if (!strncmp(buf, "findval ", 8)) {
            /* Every float equal to <val> (tolerance 1e-4) in the app's heap.
             * Run at two knob positions and intersect to find the stored trim. */
            float want = (float)atof(buf + 8);
            ranges_load();
            unsigned char *chunk = malloc(65536);
            int hits = 0;
            for (int r = 0; r < g_nrng && chunk && hits < 200; ++r) {
                if (!g_rng[r].w)
                    continue;
                /* skip the image and the big audio buffers */
                if (g_rng[r].lo >= base && g_rng[r].hi <= base + 0x9000000)
                    continue;
                if (g_rng[r].hi - g_rng[r].lo > 0x4000000)
                    continue;
                for (uintptr_t a = g_rng[r].lo; a + 4 < g_rng[r].hi; a += 65536) {
                    size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
                    n &= ~(size_t)3;
                    if (!rd(a, chunk, n))
                        break;
                    for (size_t o = 0; o + 4 <= n && hits < 200; o += 4) {
                        float f;
                        memcpy(&f, chunk + o, 4);
                        float d = f - want;
                        if (d < 0.0001f && d > -0.0001f) {
                            mlog("minimod: fv %#llx %.6f\n",
                                 (unsigned long long)(a + o), (double)f);
                            hits++;
                        }
                    }
                }
            }
            free(chunk);
            mlog("minimod: fv done %d\n", hits);
        }
        else if (!strncmp(buf, "reg ", 4)) {
            g_reg_base = (uintptr_t)strtoull(buf + 4, 0, 16);
            g_reg_have = rd(g_reg_base, g_reg_buf, sizeof g_reg_buf) ? 1 : 0;
            mlog("minimod: region mark %p ok=%d\n", (void *)g_reg_base, g_reg_have);
        } else if (!strncmp(buf, "regdiff", 7)) {
            if (g_reg_have) {
                unsigned char *cur = malloc(sizeof g_reg_buf);
                if (cur && rd(g_reg_base, cur, sizeof g_reg_buf)) {
                    int shown = 0;
                    for (size_t i = 0; i + 4 <= sizeof g_reg_buf && shown < 30; i += 4) {
                        if (memcmp(cur + i, g_reg_buf + i, 4) == 0)
                            continue;
                        float a, b;
                        memcpy(&a, g_reg_buf + i, 4);
                        memcpy(&b, cur + i, 4);
                        mlog("minimod: reg %p+%#zx %.4f -> %.4f\n",
                             (void *)g_reg_base, i, (double)a, (double)b);
                        shown++;
                    }
                    mlog("minimod: regdiff done\n");
                }
                free(cur);
            }
        } else if (!strncmp(buf, "poke ", 5)) {
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
                    mlog("minimod: pair at %p (prev %.4f next %.4f)\n",
                         (void *)(a + o - 4), (double)prev, (double)v);
                }
            }
        }
    }
    mlog("minimod: pair search done, %d hits\n", shown);
    free(buf);
}

/* Byte-level snapshot of the small writable ranges: a knob move changes the
 * pad's stored trim, and this names it exactly. */
#define BS_MAX (48u << 20)
static unsigned char *g_bs;
static size_t g_bs_len;

/* "findv <float>" - every address in the small writable ranges holding exactly
 * this float. Run with two knob values and intersect to find the stored state. */
static void byte_snap(void)
{
    ranges_load();
    free(g_bs);
    g_bs = 0;
    g_bs_len = 0;
    size_t cap = BS_MAX, used = 0;
    unsigned char *buf = malloc(cap);
    if (!buf)
        return;
    for (int r = 0; r < g_nrng; ++r) {
        if (!g_rng[r].w)
            continue;
        if (g_rng[r].lo >= base && g_rng[r].hi <= base + 0x9000000)
            continue;
        size_t len = (size_t)(g_rng[r].hi - g_rng[r].lo);
        if (len > 0x400000)      /* skip the big audio buffers */
            continue;
        if (used + 16 + len > cap)
            break;
        uint64_t addr = g_rng[r].lo;
        memcpy(buf + used, &addr, 8);
        memcpy(buf + used + 8, &len, 8);
        if (!rd(g_rng[r].lo, buf + used + 16, len)) {
            continue;
        }
        used += 16 + len;
    }
    g_bs = buf;
    g_bs_len = used;
    mlog("minimod: byte snapshot %zu bytes\n", used);
}

static void byte_diff(void)
{
    if (!g_bs)
        return;
    unsigned char *cur = malloc(0x400000);
    if (!cur)
        return;
    int shown = 0;
    size_t o = 0;
    while (o + 16 <= g_bs_len && shown < 24) {
        uint64_t addr, len;
        memcpy(&addr, g_bs + o, 8);
        memcpy(&len, g_bs + o + 8, 8);
        if (o + 16 + len > g_bs_len)
            break;
        if (len <= 0x400000 && rd((uintptr_t)addr, cur, len)) {
            for (size_t i = 0; i + 4 <= len && shown < 24; i += 4) {
                if (memcmp(cur + i, g_bs + o + 16 + i, 4) == 0)
                    continue;
                float a, b;
                memcpy(&a, g_bs + o + 16 + i, 4);
                memcpy(&b, cur + i, 4);
                if (!(a > -2.0f && a < 2.0f) && !(b > -2.0f && b < 2.0f))
                    continue;
                mlog("minimod: bdiff %#llx %.4f -> %.4f\n",
                     (unsigned long long)(addr + i), (double)a, (double)b);
                shown++;
            }
        }
        o += 16 + len;
    }
    free(cur);
    mlog("minimod: byte diff done\n");
}

/* Probe: "who <addr>" - every object that holds a pointer to <addr>, with the
 * holder's offset and whether it looks like an object (vtable in the image). */
static void who_points_at(uintptr_t target)
{
    ranges_load();
    unsigned char *buf = malloc(65536);
    if (!buf)
        return;
    int shown = 0;
    for (int r = 0; r < g_nrng && shown < 20; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && shown < 20; a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && shown < 20; o += 8) {
                uintptr_t q;
                memcpy(&q, buf + o, 8);
                if (q != target)
                    continue;
                uintptr_t holder = a + o;
                uintptr_t vt = 0;
                rd(a, &vt, 8); /* object base in this chunk */
                int base_off = (int)(holder & 0xffff);
                mlog("minimod: who %p holder %p chunkvt=%p off_in_chunk=%#x\n",
                     (void *)target, (void *)holder, (void *)vt, base_off);
                shown++;
            }
        }
    }
    mlog("minimod: who done, %d holders\n", shown);
    free(buf);
}

/* Probe: /run/minimod.bcast holds "i,v" - broadcast g_bp[i] with value v, so we
 * can find which recorded property drives the page. */
static void *bcast_probe_thread(void *arg)
{
    (void)arg;
    struct timespec ts = {.tv_sec = 0, .tv_nsec = 300 * 1000 * 1000};
    int last_i = -1;
    float last_v = -1;
    for (;;) {
        nanosleep(&ts, 0);
        int fd = open("/run/minimod.bcast", O_RDONLY);
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
        mlog("minimod: bcast probe %d (%p) = %.4f\n", idx, (void *)g_bp[idx],
             (double)val);
        FN(A_BROADCAST, void (*)(void *, float))((void *)g_bp[idx], val);
    }
    return 0;
}

/* Re-arms the software watchpoint from /run/minimod.watch (hex address), so the
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
        int fd = open("/run/minimod.watch", O_RDONLY);
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
            mlog("minimod: %d screen instance(s)\n", ns);
            for (int i = 0; i < ns; ++i) {
                mlog("minimod:   screen[%d]=%p\n", i, (void *)sc[i]);
                watch_arm(sc[i] + 0x1f4c, "screen");
                watch_arm(sc[i] + 0x72c8, "screenfrac");
            }
            unlink("/run/minimod.watch");
            continue;
        }
        uintptr_t a = (uintptr_t)strtoull(buf, 0, 16);
        if (a)
            watch_arm(a, "addr");
    }
    return 0;
}

static int find_main(struct dl_phdr_info *info, size_t size, void *out)
{
    (void)size;
    mlog("minimod: entry '%s' addr=%p phnum=%u\n", info->dlpi_name ? info->dlpi_name : "",
         (void *)info->dlpi_addr, (unsigned)info->dlpi_phnum);
    for (unsigned i = 0; i < info->dlpi_phnum; ++i) {
        const ElfW(Phdr) *p = &info->dlpi_phdr[i];
        if (p->p_type == PT_LOAD)
            mlog("minimod:   PT_LOAD vaddr=%p memsz=%p filesz=%p\n",
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
#if MINIMOD_LOG
    log_fd = open("/run/minimod.log", O_WRONLY | O_CREAT | O_APPEND, 0644);
#endif
#if MINIMOD_DEBUG
    /* Debug builds only: the watch probe needs to see the fault it caused.
     * Never installed in release - swallowing a SIGSEGV/SIGBUS changes the
     * host app's behaviour on every fault. */
    {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = fault_handler;
        sa.sa_flags = SA_SIGINFO;
        sigemptyset(&sa.sa_mask);
        sigaction(SIGSEGV, &sa, 0);
        sigaction(SIGBUS, &sa, 0);
    }
#endif
    uintptr_t found = 0;
    if (!dl_iterate_phdr(find_main, &found)) {
        mlog("minimod: stock app not found; not installing\n");
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
        mlog("minimod: relay mmap near base failed\n");
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
        mlog("minimod: trampoline mmap near base failed\n");
        return;
    }
    mlog("minimod: relay at %p (site %p)\n", relay_page, (void *)(base + A_SIXTEEN_PROC));
    const char *env = getenv("MINIMOD_HOOKS");
    if (env && *env)
        hook_mask = atoi(env);
    const char *safe_env = getenv("MINIMOD_SAFE");
    const char *dbg_env = getenv("MINIMOD_DEBUG");
    safe_mode = safe_env && atoi(safe_env) != 0;
    debug_mode = dbg_env && atoi(dbg_env) != 0;
    const char *probe_env = getenv("MINIMOD_PROBE");
    probe_mode = probe_env && atoi(probe_env) != 0;
    const char *bits_env = getenv("MINIMOD_PROBES");
    if (bits_env && *bits_env)
        probe_bits = atoi(bits_env);
    void *t_ctor = (hook_mask & 1) ? install_hook(base + A_SIXTEEN_CTOR, (void *)sixteen_ctor) : 0;
    void *t_proc = (hook_mask & 2) ? install_hook(base + A_SIXTEEN_PROC, (void *)sixteen_proc) : 0;
    void *t_sync = install_hook(base + 0x2822260, (void *)sync_probe);
    if (t_sync)
        orig_sync = (void (*)(void *, void *, void *, void *))t_sync;
    void *t_data = install_hook(base + A_DATA_SIGNAL, (void *)data_signal);
    if (t_data)
        orig_data_signal = (void (*)(void *, int))t_data;
    void *t_cc = install_hook(base + 0x159f230, (void *)cc_capture);
    if (t_cc)
        orig_cc = (void (*)(void *, unsigned, float, float))t_cc;
    void *t_loc = install_hook(base + A_LOC_BOUNDS, (void *)loc_bounds);
    if (t_loc)
        orig_loc_bounds = (void (*)(void *))t_loc;
    void *t_poff = install_hook(base + A_PITCH_OFF, (void *)pitch_off);
    if (t_poff)
        orig_pitch_off = (void (*)(void *, uint64_t, uint64_t, int))t_poff;
    void *t_midi = install_hook(base + A_MIDI_NOTE_EVENT, (void *)midi_note_event);
    if (t_midi)
        orig_midi_note_event = (void (*)(void *, void *, void *, uintptr_t))t_midi;
    void *t_btn = (hook_mask & 16) ? install_hook(base + A_BUTTONS, (void *)buttons) : 0;
    void *t_note = (hook_mask & 32) ? install_hook(base + A_NOTE_ON, (void *)note_on) : 0;
    void *t_cs = (hook_mask & 64) ? install_hook(base + A_CS_EVENT, (void *)cs_event) : 0;
    void *t_off = (hook_mask & 128) ? install_hook(base + A_NOTE_OFF, (void *)note_off) : 0;
    void *t_bass_audio = install_hook(base + A_BASS_AUDIO_WRITER, (void *)bass_audio_writer);
    void *t_sc = install_hook(base + 0x1968020, (void *)setctl_probe);
    if (t_sc)
        orig_setctl = (void (*)(void *, void *, int, int, float))t_sc;
#if MINIMOD_DEBUG
    void *t_pg = install_hook(base + 0x28100c0, (void *)pagector_probe);
    if (t_pg)
        orig_pagector = (pagector_fn)t_pg;
    void *t_ch = install_hook(base + 0x2804e90, (void *)ctlhandler_probe);
    if (t_ch)
        orig_ctlhandler = (void (*)(void *, uint64_t, uint64_t))t_ch;
    void *t_gt = install_hook(base + 0x2812f10, (void *)gettrim_probe);
    if (t_gt)
        orig_gettrim = (void *(*)(void *))t_gt;
    void *t_cp = install_hook(base + 0x280dca0, (void *)copypad_probe);
    if (t_cp)
        orig_copypad = (void (*)(void *, void *))t_cp;
#endif
#if MINIMOD_DEBUG
    {
        void *t = 0;
        t = install_hook(base + 0x20a3b10, (void *)add_slice_hook);
        if (t)
            orig_add_slice = (void (*)(void *, void *, uint64_t))t;
        t = install_hook(base + 0x20a9fa0, (void *)add_slice_ph_hook);
        if (t)
            orig_add_slice_ph = (void (*)(void *))t;
        t = install_hook(base + 0x1960af0, (void *)insert_cmd_hook);
        if (t)
            orig_insert_cmd = (void (*)(void *, void *, unsigned, void *))t;
        t = install_hook(base + 0x195cf40, (void *)rmall_cmd_hook);
        if (t)
            orig_rmall_cmd = (void (*)(void *, void *, void *))t;
        t = install_hook(base + 0x192d040, (void *)slice_publish_hook);
        if (t)
            orig_slice_publish = (void (*)(void *))t;
        t = install_hook(base + 0x2941ff0, (void *)slice_list_hook);
        if (t)
            orig_slice_list = (void *(*)(void *))t;
        t = install_hook(base + 0x20a7140, (void *)slice_action_hook);
        if (t)
            orig_slice_action = (void (*)(void *, void *, int))t;
        t = install_hook(base + 0x1960d50, (void *)split_cmd_hook);
        if (t)
            orig_split_cmd = (void (*)(void *, void *, unsigned, void *))t;
        t = install_hook(base + 0x195d1c0, (void *)rm_slice_hook);
        if (t)
            orig_rm_slice = (void (*)(void *, void *, unsigned, void *))t;
        t = install_hook(base + 0x2814ec0, (void *)marker_at_hook);
        if (t)
            orig_marker_at = (int (*)(void *, unsigned))t;
        t = install_hook(base + 0x20aa9e0, (void *)slice_sel_hook);
        if (t)
            orig_slice_sel = (void (*)(void *, unsigned))t;
        mlog("chop hooks: add=%p ph=%p cmd=%p\n", (void *)orig_add_slice,
             (void *)orig_add_slice_ph, (void *)orig_insert_cmd);
    }
#endif
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
    if (t_bass_audio) {
        orig_bass_audio_writer = (void (*)(void *, void *, size_t, int))t_bass_audio;
    }
    g_bass_preview_available = t_note && t_off && t_bass_audio;
    if (t_trim)
        orig_trim = (void (*)(void *, float))t_trim;
    if (t_sa)
        orig_str_assign = (void (*)(void *, const char *))t_sa;
    if (t_lt)
        orig_label_text = (void *(*)(void *))t_lt;
    if (t_dt)
        orig_draw_text = (void (*)(void *, const uint64_t *, int, int, int, int, int, int, float))t_dt;
    {
        void *t = 0;
        t = install_hook(base + A_HPSS_EXTRACT_CTOR, (void *)hpss_ctor_hook);
        if (t)
            orig_hpss_ctor = (void (*)(void *, void *, int, void *, void *, void *,
                                       void *, void *, void *))t;
        t = install_hook(base + A_HPSS_EXTRACT_EXEC, (void *)hpss_exec_hook);
        if (t)
            orig_hpss_exec = (void (*)(void *))t;
        t = install_hook(base + A_HPSS_WRITE_CB, (void *)hpss_writer_hook);
        if (t)
            orig_hpss_writer = (void (*)(void *, void *))t;
        mlog("HPSS hooks: ctor=%p exec=%p writer=%p\n",
             (void *)orig_hpss_ctor, (void *)orig_hpss_exec,
             (void *)orig_hpss_writer);
    }
    install_screen_paint();
#if MINIMOD_DEBUG
    {
        pthread_t wth, sth, bth, hth, cth;
        pthread_create(&wth, 0, watch_thread, 0);
        pthread_create(&sth, 0, slider_probe_thread, 0);
        pthread_create(&bth, 0, bcast_probe_thread, 0);
        pthread_create(&hth, 0, heap_probe_thread, 0);
        pthread_create(&cth, 0, chop_probe_thread, 0);
    }
#endif
    if (!relay_used) {
        mlog("minimod: no hook installed\n");
        return;
    }
    /* Hardening failures must never take the host app down: leave the pages
     * writable and keep running unhooked rather than exit. */
    if (mprotect(relay_page, 4096, PROT_READ | PROT_EXEC) != 0) {
        mlog("minimod: relay mprotect failed: %s (continuing)\n", strerror(errno));
        return;
    }
    if (mprotect(tramp_page, 4096, PROT_READ | PROT_EXEC) != 0) {
        mlog("minimod: tramp mprotect failed: %s (continuing)\n", strerror(errno));
        return;
    }
    mlog("minimod: codes relay=%p tramp=%p\n", (void *)relay_page,
         (void *)tramp_page);
    mlog("minimod: base=%p mask=%d safe=%d probe=%d (ctor=%d proc=%d plus=%d minus=%d)\n",
         (void *)base, hook_mask, safe_mode, probe_mode, t_ctor != 0, t_proc != 0);
}
