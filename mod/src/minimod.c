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
#include "hooks_ffi.h"
#include "levels_ffi.h"
#include "hooks_ffi.h"
#include "page_ffi.h"
#include "selection_ffi.h"
#include "chop_ffi.h"
#include "pads_ffi.h"
#include "cfg_ffi.h"
#include "sample_ffi.h"
#include "status_ffi.h"
#include "paths_ffi.h"
#include "wav_ffi.h"
#include "bass_sample.h"
#include "looper.h"

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
static uintptr_t base;
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
#include "clock_ffi.h"
static void *g_screen;
static void looper_draw(void *g);
static void looper_pump(void);
static int prop_references(uintptr_t P, uintptr_t param);

#define MAXW 12





static volatile int g_cc_recover;
static sigjmp_buf g_cc_jmp;

#define LOG_CAP (256u * 1024u)

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


/* Each hook saves the caller's stack pointer in the relay slot and the
 * trampoline restores it before running the displaced instruction, so the
 * original function continues on its own frame. */
/* The hook is entered instead of the function, and calls the trampoline to run
 * the original. The trampoline holds a copy of the function's prologue (so the
 * body runs on its own frame) and then branches into the body; the body's
 * epilogue returns straight to the hook, which returns to the app with all
 * callee-saved registers intact. */
/* First instruction of every site this build hooks, read from firmware
 * 1.3.0.12. If any of them does not match, nothing is installed: patching an
 * offset in a different build would overwrite a random instruction. */
static int firmware_ok(void)
{
    static const struct {
        uintptr_t off;
        uint32_t word;
    } sites[] = {
        {0x00f535c0, 0xd503233f}, {0x01149880, 0xd503233f},
        {0x013bb930, 0xd503233f}, {0x0140dbc0, 0xd503233f},
        {0x0143ed40, 0xd503233f}, {0x0143f0b0, 0xd503233f},
        {0x014a7580, 0xd503233f}, {0x01545a50, 0xd503233f},
        {0x0159f230, 0x39402003}, {0x0159f2f0, 0xd503245f},
        {0x015e70c0, 0xd503233f}, {0x016b6060, 0xd503245f},
        {0x0192d040, 0xd503233f}, {0x0195cf40, 0xd503233f},
        {0x0195d1c0, 0xd503233f}, {0x01960af0, 0xd503233f},
        {0x01960d50, 0xd503233f}, {0x01968020, 0xd503233f},
        {0x01d8aff0, 0xd503233f}, {0x01e270d0, 0xd503233f},
        {0x01e27360, 0xd503233f}, {0x020a3b10, 0xd503233f},
        {0x020a7140, 0xd503233f}, {0x020a9fa0, 0xd503233f},
        {0x020aa9e0, 0xd503233f}, {0x0211b090, 0xd503233f},
        {0x021472f0, 0xd503233f}, {0x023aff60, 0xd503233f},
        {0x027e4400, 0xd503233f}, {0x02804e90, 0xd503233f},
        {0x0280dca0, 0xd503233f}, {0x028100c0, 0xd503233f},
        {0x02812f10, 0xd503245f}, {0x02814ec0, 0xd503233f},
        {0x02822260, 0xd503233f}, {0x02941ff0, 0xd503245f},
        {0x03f398f0, 0xd503233f},
    };
    for (unsigned i = 0; i < sizeof sites / sizeof sites[0]; ++i) {
        uint32_t w = 0;
        if (!rd(base + sites[i].off, &w, 4) || w != sites[i].word) {
            mlog("minimod: site %#lx is %08x, expected %08x - firmware mismatch\n",
                 (unsigned long)sites[i].off, w, sites[i].word);
            return 0;
        }
    }
    return 1;
}

/* The engine that maps the pages, displaces the prologue and patches the site
 * lives in Rust; this is only the C name the constructor calls. */
static void *install_hook(uintptr_t site, void *hook)
{
    return minimod_install_hook(base, site, (uintptr_t)hook);
}

/* ---------------------------------------------------------- MIDI hook (lazy)
 * Outside 16 Levels the stock app must see its own dispatcher code, not a
 * hook that merely forwards. The slot and the trampoline are built once; the
 * entry instruction is swapped in when 16 Levels turns on and restored when
 * it turns off or the Sample page goes away. */
static void (*orig_midi_note_event)(void *, void *, void *, uintptr_t);
static void midi_note_event(void *app, void *context, void *event,
                            uintptr_t source);
static int g_midi_ready, g_midi_on;
/* A physical pad press identifies its pad through property 0 of the note event
 * (event+0x16), whose value is the bank's pad-note base plus the pad index.
 * Learn that base from a real pad press so MIDI can address the same row. */
static volatile int g_pad_prop_pending, g_pad_prop_val, g_pad_prop_base = 0x75;
static volatile int g_pad_level_byte;
static volatile uintptr_t g_pad_ctx;
static volatile int g_pad_tune_mode;
static unsigned char g_midi_replayed[128];

static int midi_hook_site_ok(void)
{
    return minimod_midi_site_ok(base);
}

static void midi_hook_set(int on)
{
    /* The dispatcher is hooked once at load; this only changes behaviour, so no
     * code is rewritten while the MIDI thread may be running it. */
    g_midi_on = on ? 1 : 0;
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
    return minimod_page_chop_mode(base, (uintptr_t)g_screen);
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
        if (g_levels) mlog("lv: predicate 0, clearing\n");
        g_levels = 0;
        midi_hook_set(0);
        return 0;
    }
    return g_levels;
}


static int bump_octave(int d)
{
    int next = 0;
    if (!minimod_levels_shift(g_octave, d, &next))
        return 0;
    g_octave = next;
    char buf[48];
    snprintf(buf, sizeof buf, "Pitch %+d semitones", next * 8);
    toast(next == 0 ? "Pitch reset" : buf);
    return 1;
}



/* SHIFT+B3 cycles 3 % -> 6 % -> 9 % -> untrimmed while SHIFT stays held;
 * releasing SHIFT resets the cycle. */
static int g_trim_step;
static double g_trim_ratio;
static int g_b3_down;      /* edge detection: the button auto-repeats while held */












/* The page binds each trim parameter through a small binding object whose
 * vtable is app+0x6e08a10 and which keeps the property pointer at +0x20. */
#define A_BINDING_VT 0x6e08a10


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




/* ------------------------------------------------------------- app hooks */
static uintptr_t g_trim_layer; /* layer the trim page is editing */
static void *(*orig_sixteen_ctor)(void *, void *, void *, void *);
static void (*orig_sixteen_proc)(void *, int, void *);
static void (*orig_pitch_off)(void *, uint64_t, uint64_t, int);
static void (*orig_buttons)(void *);
static void (*orig_note_on)(void *, int, int, void *, float);
static void (*orig_note_off)(void *, int, int);
static void (*orig_midi_note_event)(void *, void *, void *, uintptr_t);
static volatile uint32_t g_midi_chromatic_count;
static volatile int g_midi_chromatic_note = -1;
static volatile int g_midi_chromatic_delta;
static void (*orig_bass_audio_writer)(void *, void *, size_t, int);
static volatile unsigned g_audio_in_calls;
static volatile unsigned g_audio_out_calls;
static void (*orig_cs)(void *, int, void *);
static void (*orig_trim)(void *, float);
/* The Sample page (0x2468 bytes) captured from SamplePage_Ctor; it owns the
 * Start/End widgets and is what must be repainted after a property change. */
static void *g_page;
typedef void (*pagector_fn)(void *, void *, void *, void *, void *, void *,
                            void *, void *, void *, void *, void *, void *);


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


/* FUN_02912f10(padBlock) returns the object the page reads its trim from
 * (Start at +0x78, End at +0xd0). Capturing it in-process avoids stale pointers. */


/* PadState_CopyIntoPage(page, padState): the pad is the source of the page's
 * Start/End widgets (pad+0x1588 -> widget value, pad+0x15e0 -> End). */


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
static volatile int g_erase_down;
static volatile int g_buf_dirty; /* re-read the device rings on the audio thread */
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
    int off = minimod_levels_pitch_offset(a3, g_octave);
    if (g_octave && debug_mode)
        mlog("minimod: pitch offset %d -> %d\n", a3, off);
    if (orig_pitch_off)
        orig_pitch_off(a0, a1, a2, off);
}

static uintptr_t g_last_press_obj;
/* "findv <float>" - every address in the small writable ranges holding exactly
 * this float. Run with two knob values and intersect to find the stored state. */

static void (*orig_screen_paint)(void *, void *);
static volatile int g_pending_ctl;
static float g_pending_v0, g_pending_v1;
static int g_pending_r0, g_pending_r1;
static void (*orig_setctl2)(void *, void *, int, int, float);
static void *g_surface2;


/* Write the property values immediately before the app paints, so the paint
 * reads the trim we computed. */

static void *g_paint_comp;
static void *g_paint_gfx;
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
                g_hpss_sel = (g_hpss_sel + (delta > 0 ? 1 : 7)) % 8;
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
                            unsigned *rate)
{
    uintptr_t found_screen = 0, found_state = 0, found_sample = 0;
    unsigned found_frames = 0, found_ch = 0, found_rate = 0;
    if (!minimod_sample_now(base, (uintptr_t)s, &found_screen, &found_state,
                            &found_sample, &found_frames, &found_ch,
                            &found_rate))
        return 0;
    if (screen) *screen = (void *)found_screen;
    if (state) *state = found_state;
    if (sample) *sample = found_sample;
    if (frames) *frames = found_frames;
    if (ch) *ch = found_ch;
    if (rate) *rate = found_rate;
    return 1;
}

static int hpss_sample_now(void **screen, uintptr_t *state, uintptr_t *sample,
                           unsigned *frames, unsigned *ch, unsigned *rate)
{
    return hpss_read_sample(g_screen, screen, state, sample, frames, ch, rate);
}







/* Count empty pads with the same two checks the native finder uses: a null pad
 * state, or a pad state whose layers are all empty. */

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
    int before = minimod_pads_free_pad(base, (uintptr_t)program);
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
        after = minimod_pads_free_pad(base, (uintptr_t)program);
    }
    int write_ok = buf ? g_hpss_write_ok : 1;
    int ok = after != before && write_ok;
    mlog("HPSS extract: pad %d -> free %d (%s, write %s) range %u..%u\n", before,
         after, after != before ? "assigned" : "NO ASSIGNMENT",
         write_ok ? "complete" : "INCOMPLETE", start, end);
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
static unsigned char g_hpss_btn[16];
static int g_hpss_btn_valid;
static int g_hpss_chop_before;
static int g_hpss_restore_pending;
static void hpss_repaint(void)
{
    void *comp = g_paint_comp ? g_paint_comp : g_screen;
    if (orig_repaint && comp)
        orig_repaint(comp, 0, ((uint64_t)2000 << 32) | (uint64_t)2000);
}

static int sample_page_visible(void)
{
    return minimod_page_visible(base, (uintptr_t)g_screen);
}

static int hpss_chop_mode(void)
{
    return minimod_page_chop_mode(base, (uintptr_t)g_screen);
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
    if (!rd(state + 0x2310, &program, 8) || !program || minimod_pads_free_estimate(base, (uintptr_t)program) < 1) {
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
    return minimod_spectral_manual(base, (uintptr_t)g_screen);
}


static int spectral_write_markers(void)
{
    return minimod_spectral_write_markers(base, g_spectral.sample, g_spectral.pos,
                                          (unsigned)g_spectral.n, g_spectral.frames,
                                          (uintptr_t)g_screen);
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
        g_spectral.deadline = minimod_now_ms() + 3000;
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
            if (minimod_now_ms() >= g_spectral.deadline)
                spectral_finish("Manual chop did not open");
        } else if (g_spectral.phase == 1) {
            if (!spectral_write_markers()) {
                spectral_finish("native marker write failed");
                return;
            }
            g_spectral.phase = 2;
            g_spectral.deadline = minimod_now_ms() + 150;
        } else if (minimod_now_ms() >= g_spectral.deadline) {
            spectral_finish(minimod_spectral_verify(g_spectral.sample, g_spectral.pos, (unsigned)g_spectral.n, g_spectral.frames) ? NULL : "marker verification failed");
        }
    }
    hpss_repaint();
}


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
            minimod_pads_free_estimate(base, (uintptr_t)program) < required) {
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
            /* Only the bass path takes a source backup (fmbass_start); every
             * other action leaves the source file untouched, so there is
             * nothing to restore.  Demanding a restore here failed every
             * effect with "source restore failed". */
            int source_ok = !job->source_audio_backup ||
                            bass_source_restore(job->source_path,
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
    else if (g_hpss_sel == 6) {
        if (looper_activate()) {
            g_buf_dirty = 1; /* re-read the device rings on its own thread */
            g_hpss_menu = 0;
            g_bass_edit = 0;
            bass_preview_set_active(0);
            looper_pump();
        }
        hpss_repaint();
    }
    else
        hpss_close();
}

static void draw_colour(void *g, uint32_t v);
static void draw_rect(void *g, int x, int y, int w, int h);
static void draw_text(void *g, const char *s, int x, int y, int w, int h,
                      float size);

/* ------------------------------------------------------------ live looper */
/* The view is a fullscreen overlay: the stock page paints first, then this
 * covers the whole 320x240 area, so the looper really is a separate screen.
 * A small thread keeps it animating while it is open, the same way the toast
 * forces its own repaints. */
static volatile int g_looper_pump_started;

static void looper_cfg_save(void);
static long long g_cfg_saved_at;

/* Buffer geometry of the two streams, read once from ALSA. The firmware keeps
 * both of its rings full, so one buffer per direction is the latency it works
 * with - the number the sampler itself uses, no probe needed. */
static volatile unsigned g_buf_out;
static volatile unsigned g_buf_in;
static volatile unsigned g_out_ch; /* channels of the device buffer, from ALSA */
static volatile unsigned g_in_ch;
static volatile int g_out_s32;      /* output stream is S32_LE */
static volatile int g_in_s32;       /* input stream is S32_LE */
static unsigned stream_geometry(void *pcm, unsigned *channels, int *s32);

static void *looper_pump_thread(void *arg)
{
    (void)arg;
    for (;;) {
        struct timespec ts = {0, 60 * 1000 * 1000};
        nanosleep(&ts, 0);
        if (!looper_is_open())
            continue;
        /* The counters move in bursts inside each audio callback, so a single
         * sample depends on the phase. Average a short burst of samples that
         * spans several callbacks to get the level itself. */
        void *comp = g_paint_comp ? g_paint_comp : g_screen;
        if (orig_repaint && comp && looper_is_open()) /* do not paint into a view being closed */
            orig_repaint(comp, 0, ((uint64_t)2000 << 32) | (uint64_t)2000);
    }
    return 0;
}

static void looper_pump(void)
{
    if (__atomic_load_n(&g_looper_pump_started, __ATOMIC_ACQUIRE))
        return;
    __atomic_store_n(&g_looper_pump_started, 1, __ATOMIC_RELEASE);
    pthread_t th;
    if (pthread_create(&th, 0, looper_pump_thread, 0) == 0)
        pthread_detach(th);
}

#define LP_MAX_HINT (44100u * 8u)

static void looper_cell(void *g, int x, int y, int w, int h,
                        const struct looper_pad_view *pad, int index,
                        int recording)
{
    uint32_t bg = 0xff1a1a20, border = 0xff33333c, text = 0xff8a8a92;
    const char *state = "--";
    switch (pad->state) {
    case LOOP_REC:
        bg = 0xff3a1010; border = 0xffff4d4d; text = 0xffffb0b0; state = "REC";
        break;
    case LOOP_PLAY:
        bg = 0xff10301a; border = 0xff54d06a; text = 0xffc8f5d0; state = "PLAY";
        break;
    case LOOP_DUB:
        bg = 0xff352808; border = 0xffffb43c; text = 0xffffe0a8; state = "DUB";
        break;
    case LOOP_STOP:
        bg = 0xff202028; border = 0xff6a6a74; text = 0xffc0c0c8; state = "STOP";
        break;
    }
    draw_colour(g, bg);
    draw_rect(g, x, y, w, h);
    draw_colour(g, border);
    draw_rect(g, x, y, w, 1);
    draw_rect(g, x, y + h - 1, w, 1);
    draw_rect(g, x, y, 1, h);
    draw_rect(g, x + w - 1, y, 1, h);
    if (recording) {
        draw_colour(g, 0xffffffff);
        draw_rect(g, x + 1, y + 1, w - 2, 1);
    }
    draw_colour(g, text);
    char label[8];
    snprintf(label, sizeof label, "%d", index + 1);
    draw_text(g, label, x + 4, y + 2, 18, 12, 10.0f);
    draw_text(g, state, x + 20, y + 2, w - 22, 12, 9.0f);
    /* bottom strip: loop length (or captured amount while recording) */
    unsigned total = pad->state == LOOP_REC ? (unsigned)(LP_MAX_HINT) : pad->len_frames;
    unsigned done = pad->state == LOOP_REC ? pad->rec_frames : pad->pos;
    draw_colour(g, 0xff2a2a32);
    draw_rect(g, x + 4, y + h - 9, w - 8, 4);
    if (total) {
        unsigned pct = (unsigned)((uint64_t)done * (w - 8) / total);
        if (pct > (unsigned)(w - 8))
            pct = (unsigned)(w - 8);
        draw_colour(g, pad->state == LOOP_REC ? 0xffff6a6a : 0xff54d06a);
        draw_rect(g, x + 4, y + h - 9, (int)pct, 4);
    }
    if (pad->state != LOOP_REC && pad->len_frames) {
        char len[12];
        snprintf(len, sizeof len, "%.1fs", (double)pad->len_frames / 44100.0);
        draw_colour(g, 0xffb8b8be);
        draw_text(g, len, x + 4, y + 15, w - 8, 11, 8.0f);
    }
}

static void looper_draw(void *g)
{
    struct looper_view v;
    if (!looper_snapshot(&v))
        return;
    draw_colour(g, 0xff0c0c10);
    draw_rect(g, 0, 0, 320, 240);
    draw_colour(g, 0xff15151a);
    draw_rect(g, 0, 0, 320, 24);
    draw_colour(g, 0xffffb43c);
    draw_rect(g, 0, 24, 320, 2);
    draw_colour(g, 0xffffffff);
    draw_text(g, "LIVE LOOPER", 8, 2, 180, 20, 15.0f);
    /* sync pulse state and beat dot */
    if (v.sync && v.sync_frames) {
        char line[24];
        snprintf(line, sizeof line, "SYNC %.2fs%s",
                 (double)v.sync_frames / 44100.0, v.click ? " +CLK" : "");
        draw_colour(g, 0xffffb43c);
        draw_text(g, line, 200, 4, 100, 16, 10.0f);
        int on = v.pulse_phase != 0xffffffffu &&
                 v.pulse_phase < v.sync_frames / 6;
        draw_colour(g, on ? 0xffffffff : 0xffffb43c);
        draw_rect(g, 305, 8, 8, 8);
        int pct = (int)((uint64_t)v.pulse_phase * 320 / v.sync_frames);
        if (pct < 0)
            pct = 0;
        if (pct > 320)
            pct = 320;
        draw_colour(g, 0xff3a2a10);
        draw_rect(g, 0, 236, 320, 3);
        draw_colour(g, 0xffffb43c);
        draw_rect(g, 0, 236, pct, 3);
    } else {
        draw_colour(g, 0xff8a8a92);
        draw_text(g, "FREE", 248, 4, 60, 16, 10.0f);
    }
    /* input meter */
    draw_colour(g, 0xff8a8a92);
    draw_text(g, "IN", 4, 27, 16, 10, 7.0f);

    draw_colour(g, 0xff2a2a32);
    draw_rect(g, 22, 30, 290, 4);
    draw_colour(g, v.input > 0.9f ? 0xffff4d4d : 0xff54d06a);
    int meter = (int)(v.input * 290.0f);
    if (meter > 290)
        meter = 290;
    if (meter > 0)
        draw_rect(g, 22, 30, meter, 4);
    /* 4x4 pad grid */
    for (int i = 0; i < LOOPER_PADS; ++i) {
        int col = i % 4, row = i / 4;
        int x = 8 + col * 78, y = 40 + row * 41;
        looper_cell(g, x, y, 75, 38, &v.pads[i], i, v.recording_pad == i);
    }
    /* status line and key hints */
    if (v.message[0]) {
        draw_colour(g, 0xff201408);
        draw_rect(g, 8, 202, 304, 12);
        draw_colour(g, 0xffffb43c);
        draw_text(g, v.message, 12, 202, 296, 12, 10.0f);
    } else if (v.lat_frames) {
        char lat[28];
        const char *src = v.lat_src == 'B' ? "BUF" : v.lat_src == 'F' ? "FIX" : "EST";
        snprintf(lat, sizeof lat, "%s %ums", src,
                 (unsigned)((uint64_t)v.lat_frames * 1000 / 44100));
        draw_colour(g, 0xff5a5a64);
        draw_rect(g, 8, 202, 304, 12);
        draw_colour(g, 0xffb8b8be);
        draw_text(g, lat, 12, 202, 296, 12, 9.0f);
    }
    draw_colour(g, 0xffb8b8be);
    draw_text(g, "PAD REC/PLAY/STOP  SHIFT OVERDUB  ERASE CLEAR", 8, 214, 304, 9, 7.0f);
    draw_text(g, "STOP ALL  LOOP SYNC  SHIFT+LOOP CLICK  SHIFT+DATA EXIT",
              8, 224, 304, 9, 7.0f);
}

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
            "Bass Synth", "Live Looper", "Close"};
        for (int i = 0; i < 8; ++i) {
            int row_y = y + 26 + i * 20;
            if (g_hpss_sel == i) {
                draw_colour(g, 0xff3a2a10);
                draw_rect(g, x + 6, row_y, w - 12, 19);
                draw_colour(g, 0xffffffff);
            }
            draw_text(g, titles[i], x + 12, row_y + 1, w - 24, 17, 12.0f);
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

static void release_held_pads(void);

static void buttons(void *event)
{

    unsigned code = event ? FIELD(event, 8, uint8_t) : 0;
    unsigned down = event ? FIELD(event, 9, uint8_t) : 0;
    mlog("btn %02x %u\n", code, down);
    /* Keep the lazy MIDI hook in sync with the visible mode: leaving the Sample
     * page restores the stock dispatcher on the very next button. */
    midi_hook_set(levels_active());
    if (looper_is_open()) {
        if (code == 0x31) {
            /* Track SHIFT here but never forward it: the app pairs SHIFT with a
             * pad press for its own shortcuts (Full Level), and the looper owns
             * the pads. */
            g_shift = down ? 1 : 0;
            return;
        }
        if (code == 0x09) { /* ERASE: held while a pad is pressed clears it */
            g_erase_down = down ? 1 : 0;
            return;
        }
        int was = looper_is_open();
        looper_button(code, (int)down, g_shift, g_erase_down);
        if (was && !looper_is_open()) {
            g_shift = 0;
            g_erase_down = 0;
            release_held_pads();
            hpss_repaint();
        }
        return;
    }
    if (event && rd((uintptr_t)event, g_hpss_btn, sizeof g_hpss_btn)) {
        g_hpss_btn_valid = 1;
        uintptr_t context = 0, surface = 0;
        memcpy(&context, g_hpss_btn, sizeof context);
        if (context && rd(context + 0x10, &surface, sizeof surface) && surface)
            __atomic_store_n(&g_data_signal, surface + 0x2da8, __ATOMIC_RELEASE);
    }
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
        if (g_levels && g_last_pad >= 0 && g_last_pad < 16)
            g_pad_level_byte = g_last_pad;
        mlog("minimod: 16 LEVELS mode -> %d\n", g_levels);
        midi_hook_set(levels_active());
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

/* Pads whose note-on the app may have seen; a note-off that arrives while the
 * looper owns the pads is replayed when the view closes. */
static unsigned char g_held_on[128];
static void *g_held_input[128];
static int g_held_src[128];

static void release_held_pads(void)
{
    for (int p = 0; p < 128; ++p) {
        if (!g_held_on[p])
            continue;
        g_held_on[p] = 0;
        if (orig_note_off)
            orig_note_off(g_held_input[p], g_held_src[p], p);
    }
}

static void note_on(void *input, int src, int pad, void *props, float vel)
{
    if (pad >= 0 && pad < 128) {
        g_held_on[pad] = 1;
        g_held_input[pad] = input;
        g_held_src[pad] = src;
    }
    if (looper_is_open() && pad >= 0) {
        if (pad < LOOPER_PADS)
            looper_pad(pad, g_shift, g_erase_down); /* the looper owns the pads */
        return;
    }
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
    if (g_pad_prop_pending) {
        g_pad_prop_base = g_pad_prop_val - pad;
        g_pad_prop_pending = 0;
    }

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
    if (pad >= 0 && pad < 128)
        g_held_on[pad] = 0;
    if (looper_is_open() && pad >= 0)
        return;
    if (g_hpss_menu && g_bass_edit && pad >= 0 && pad < 16)
        return;
    if (orig_note_off)
        orig_note_off(input, src, pad);
}

/* The firmware's audio writer accepts planar float channels, then converts
   them to S32 PCM. Mix the preview and the live-looper loops into private
   block storage so the live engine buffers are never changed. The normal
   device block is 128 frames; the lab runs a larger one, so the copy allows
   up to 4096 and falls through for anything bigger. */
static int audio_channels(void *self, int input_side)
{
    int count;
    if (input_side) {
        count = FIELD(self, 0xd4, int32_t);
        if (count < 0)
            count = FIELD(self, 0xe0, int32_t);
        if (count < 1 || count > 8) {
            count = FIELD(self, 0xd8, int32_t);
            if (count < 0)
                count = FIELD(self, 0xe4, int32_t);
        }
    } else {
        count = FIELD(self, 0xd8, int32_t);
        if (count < 0)
            count = FIELD(self, 0xe4, int32_t);
    }
    if (count < 1 || count > 8)
        count = 2;
    return count;
}

/* The firmware's input stream is RW_INTERLEAVED S32_LE. Rather than chase the
 * app's own read thread, the preloaded library exports the two ALSA entry
 * points it uses and forwards to the real libasound through dlsym. LD_PRELOAD
 * puts us ahead of libasound in the global scope, so every read in the process
 * lands here, whichever device object owns the pcm handle. */
static long (*real_snd_pcm_readi)(void *, void *, unsigned long);
static long (*real_snd_pcm_writei)(void *, const void *, unsigned long);
static long (*real_snd_pcm_delay)(void *, long *);
static long (*real_snd_pcm_avail)(void *);
static unsigned (*real_hw_malloc)(void **);
static int (*real_hw_current)(void *, void *);
static int (*real_hw_bufsize)(void *, unsigned long *);
static int (*real_hw_channels)(void *, unsigned *);
static int (*real_hw_format)(void *, int *);
static void (*real_hw_free)(void *);

/* ALSA's SND_PCM_FORMAT_S32_LE; minimod has no ALSA headers. */
#define MM_FMT_S32_LE 10

/* Frames in one ALSA ring, and the channel count the device really uses.
 * Called only from the audio thread that owns the stream. */
/* Frames in the ring, the channel count and whether the stream is S32_LE -
 * all from the stream's own hw params. Returns 0 when the device cannot be
 * queried, and callers must then leave the buffer alone. */
static unsigned stream_geometry(void *pcm, unsigned *channels, int *s32)
{
    if (!pcm || !real_hw_malloc)
        return 0;
    void *hw = 0;
    unsigned frames = 0;
    if (real_hw_malloc(&hw) < 0 || !hw)
        return 0;
    if (real_hw_current(pcm, hw) >= 0) {
        unsigned long f = 0;
        if (real_hw_bufsize(hw, &f) >= 0 && f < 65536)
            frames = (unsigned)f;
        if (channels && real_hw_channels) {
            unsigned c = 0;
            if (real_hw_channels(hw, &c) >= 0 && c > 0 && c <= 32)
                *channels = c;
        }
        if (s32 && real_hw_format) {
            int f = -1;
            *s32 = real_hw_format(hw, &f) >= 0 && f == MM_FMT_S32_LE;
        }
    }
    if (real_hw_free)
        real_hw_free(hw);
    return frames;
}

static volatile unsigned g_out_delay; /* last output delay sample, frames */
static volatile unsigned g_out_delay_avg; /* smoothed, what a take uses */
static volatile unsigned g_in_delay;
static int g_lat_fixed = -1; /* MINIMOD_LOOP_LAT: override the measurement */
static int g_lat_extra;      /* MINIMOD_LOOP_LAT_EXTRA: calibration trim */
static volatile unsigned g_out_peak_bits;
static volatile unsigned g_out_write_calls;
/* In-process tap of the last two seconds of written audio (mono, channel 0).
 * The lab's PCM socket does not always reflect what the app writes, so tests
 * read this instead of trusting the host-side capture. */
#define OUTTAP_FRAMES (44100u * 2u)
static volatile float g_outtap[OUTTAP_FRAMES];
static volatile unsigned g_outtap_w;


static long (*real_snd_pcm_readn)(void *, void **, unsigned long);
static int (*real_snd_pcm_set_channels)(void *, unsigned);
static volatile unsigned g_pcm_channels = 4;

int snd_pcm_hw_params_set_channels(void *params, unsigned channels)
{
    if (!real_snd_pcm_set_channels)
        real_snd_pcm_set_channels =
            (int (*)(void *, unsigned))dlsym(RTLD_NEXT, "snd_pcm_hw_params_set_channels");
    if (channels)
        g_pcm_channels = channels;
    return real_snd_pcm_set_channels ? real_snd_pcm_set_channels(params, channels) : -1;
}

long snd_pcm_writei(void *pcm, const void *buffer, unsigned long size)
{
    if (!real_snd_pcm_writei)
        real_snd_pcm_writei =
            (long (*)(void *, const void *, unsigned long))dlsym(RTLD_NEXT, "snd_pcm_writei");
    if (!real_snd_pcm_writei)
        return -1;
    int looper = looper_is_open();
    if (looper && (!g_buf_out || g_buf_dirty)) {
        unsigned c = 0;
        int s32 = 0;
        g_buf_out = stream_geometry(pcm, &c, &s32);
        if (c) {
            g_out_ch = c;
            g_out_s32 = s32;
        }
        if (g_buf_out && g_buf_in)
            g_buf_dirty = 0;
    }
    unsigned ch = g_out_ch;
    if (looper) {
        long delay = 0;
        if (!real_snd_pcm_delay)
            real_snd_pcm_delay = (long (*)(void *, long *))dlsym(RTLD_NEXT, "snd_pcm_delay");
        if (real_snd_pcm_delay && real_snd_pcm_delay(pcm, &delay) == 0 && delay >= 0) {
            g_out_delay = (unsigned)delay;
            unsigned avg = g_out_delay_avg;
            g_out_delay_avg = avg ? (avg * 7 + g_out_delay) / 8 : g_out_delay;
        }
        if (g_lat_fixed >= 0) {
            looper_set_latency_ex((unsigned)g_lat_fixed, 'F');
        } else {
            unsigned bo = g_buf_out, bi = g_buf_in;
            long total;
            char src;
            if (bo > 0 && bi > 0) {
                /* The firmware keeps one full ring in each direction; that is
                 * the latency its own sampler works with. */
                total = (long)bo + (long)bi;
                src = 'B';
            } else {
                total = (long)g_out_delay_avg + (long)size + (long)g_in_delay +
                        (long)looper_input_backlog();
                src = 'E';
            }
            total += (long)g_lat_extra;
            if (total < 0)
                total = 0;
            if (total > 16384) /* looper.c's LP_LAT_MAX */
                total = 16384;
            looper_set_latency_ex((unsigned)total, src);
        }
    }
    if (looper && buffer && size && ch && g_out_s32)
        looper_render_s32((int32_t *)buffer, ch, (unsigned)size);
#if MINIMOD_LOG
    {
        static int n;
        if (n < 6) {
            n++;
            uintptr_t lr = (uintptr_t)__builtin_return_address(0);
            mlog("minimod: WRITEI caller=%p file=%#lx size=%lu\n", (void *)lr,
                 (unsigned long)(lr - base), (unsigned long)size);
        }
    }
#endif
    long got = real_snd_pcm_writei(pcm, buffer, size);
    if (!looper || got <= 0 || !buffer)
        return got;
    {
        __atomic_add_fetch(&g_out_write_calls, 1, __ATOMIC_RELAXED);
        const int32_t *s32 = buffer;
        float peak = 0.f;
        for (long i = 0; i < got * (long)ch; ++i) {
            float v = (float)s32[i] * (1.0f / 2147483648.0f);
            float a = v < 0.f ? -v : v;
            if (a > peak)
                peak = a;
        }
        unsigned pb = g_out_peak_bits;
        float prev;
        memcpy(&prev, &pb, sizeof prev);
        if (peak > prev) {
            unsigned nb;
            memcpy(&nb, &peak, sizeof nb);
            g_out_peak_bits = nb;
        }
        unsigned w = g_outtap_w;
        for (long i = 0; i < got; ++i)
            g_outtap[(w++) % OUTTAP_FRAMES] = (float)s32[i * (long)ch] * (1.0f / 2147483648.0f);
        g_outtap_w = w;
    }
    return got;
}

long snd_pcm_readi(void *pcm, void *buffer, unsigned long size)
{
    if (!real_snd_pcm_readi)
        real_snd_pcm_readi =
            (long (*)(void *, void *, unsigned long))dlsym(RTLD_NEXT, "snd_pcm_readi");
    if (!real_snd_pcm_readi)
        return -1;
    if (looper_is_open()) {
        long avail = 0;
        if (!real_snd_pcm_avail)
            real_snd_pcm_avail = (long (*)(void *))dlsym(RTLD_NEXT, "snd_pcm_avail");
        if (real_snd_pcm_avail && (avail = real_snd_pcm_avail(pcm)) > 0)
            g_in_delay = (unsigned)avail;
    }
#if MINIMOD_LOG
    {
        static int n;
        if (n < 6) {
            n++;
            uintptr_t lr = (uintptr_t)__builtin_return_address(0);
            mlog("minimod: READI caller=%p file=%#lx size=%lu\n", (void *)lr,
                 (unsigned long)(lr - base), (unsigned long)size);
        }
    }
#endif
    if (!g_buf_in || g_buf_dirty) {
        unsigned c = 0;
        int s32 = 0;
        g_buf_in = stream_geometry(pcm, &c, &s32);
        if (c) {
            g_in_ch = c;
            g_in_s32 = s32;
        }
        if (g_buf_in && g_buf_out)
            g_buf_dirty = 0;
    }
    long got = real_snd_pcm_readi(pcm, buffer, size);
    if (got > 0) {

        __atomic_add_fetch(&g_audio_in_calls, 1, __ATOMIC_RELAXED);
        if (looper_is_open() && buffer && g_in_ch && g_in_s32)
            looper_input_s32_interleaved((const int32_t *)buffer, g_in_ch,
                                         (unsigned)got);
    }
    return got;
}

long snd_pcm_readn(void *pcm, void **bufs, unsigned long size)
{
    if (!real_snd_pcm_readn)
        real_snd_pcm_readn =
            (long (*)(void *, void **, unsigned long))dlsym(RTLD_NEXT, "snd_pcm_readn");
    if (!real_snd_pcm_readn)
        return -1;
    long got = real_snd_pcm_readn(pcm, bufs, size);
    if (got > 0) {
        __atomic_add_fetch(&g_audio_in_calls, 1, __ATOMIC_RELAXED);
        if (looper_is_open() && bufs && bufs[0] && g_in_s32)
            looper_input_s32((const int32_t *const *)bufs, 1, (unsigned)got);
    }
    return got;
}

/* Hand-edited latency trim survives a restart. */

static void looper_cfg_load(void)
{
    int trim = 0, master = -1;
    if (!minimod_cfg_load(&trim, &master))
        return;
    g_lat_extra = trim;
    if (master >= 0)
        looper_set_master((unsigned)master);
}

static void looper_cfg_save(void)
{
    minimod_cfg_save(g_lat_extra, looper_master());
}

static void audio_writer(void *self, void *input, size_t frames, int offset)
{
    __atomic_add_fetch(&g_audio_out_calls, 1, __ATOMIC_RELAXED);
#if MINIMOD_LOG
    {
        static int n;
        if (n < 4) {
            n++;
            mlog("minimod: WRITER top self=%p frames=%zu offset=%d looper=%d\n", self,
                 frames, offset, looper_is_open());
        }
    }
#endif
    static __thread float scratch[8][4096];
    float *channels[8];
    if (!__atomic_load_n(&g_bass_preview_active, __ATOMIC_ACQUIRE)) {
        bass_preview_mix(NULL, 0, 0, 44100); /* clear any ringing voices after close */
        if (orig_bass_audio_writer)
            orig_bass_audio_writer(self, input, frames, offset);
        return;
    }
    int count = audio_channels(self, 0);
    if (!orig_bass_audio_writer)
        return;
    if (count < 1 || count > 8 || frames > 4096 || !input) {
        orig_bass_audio_writer(self, input, frames, offset);
        return;
    }

#if MINIMOD_LOG
    {
        static int n;
        if (n < 6) {
            n++;
            mlog("minimod: audio out self=%p frames=%zu count=%d offset=%d\n", self,
                 frames, count, offset);
        }
    }
#endif
    float **source = input;
    for (int i = 0; i < count; ++i) {
        if (!source[i]) {
            orig_bass_audio_writer(self, input, frames, offset);
            return;
        }
        memcpy(scratch[i], source[i], frames * sizeof(float));
        channels[i] = scratch[i];
    }
    if (__atomic_load_n(&g_bass_preview_active, __ATOMIC_ACQUIRE)) {
        float peak = bass_preview_mix(channels, frames, (unsigned)count, 44100);
        if (peak > 0.f) {
            __atomic_add_fetch(&g_bass_preview_blocks, 1, __ATOMIC_RELAXED);
            uint32_t previous_bits = __atomic_load_n(&g_bass_preview_peak_bits, __ATOMIC_RELAXED);
            float previous;
            memcpy(&previous, &previous_bits, sizeof previous);
            if (peak > previous)
                __atomic_store_n(&g_bass_preview_peak_bits, bass_float_bits(peak), __ATOMIC_RELAXED);
        }
    } else {
        bass_preview_mix(NULL, 0, 0, 44100);
    }
    orig_bass_audio_writer(self, channels, frames, offset);
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
        uintptr_t sl = probe_mode ? slider_for_value(v * 100.0f) : 0;
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
    unsigned i = __atomic_fetch_add(&g_nbc, 1, __ATOMIC_RELAXED);
    g_bc[i & 255].p = (uintptr_t)self;
    g_bc[i & 255].v = v;
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
static void *g_trim_param0, *g_trim_param1;
static float g_trim_v0, g_trim_v1;
static unsigned g_trim_frames;

/* Broadcast a unique value to one candidate property: if the app's own
 * listener chain reaches the trim core for *our* parameter, that property owns
 * it. The original value is restored right after. */

/* The page keeps a small binding object per parameter (vtable app+0x6e08a10)
 * that holds the property at +0x20. Collect those as probe candidates. */

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



/* am-mod's lesson: don't write the model, drive the app's own input path.
 * The Trim knobs are control-surface events 0x10 (Start) / 0x11 (End) with an
 * absolute 7-bit position, so the app updates parameter, page and audio. */

/* am-mod sets native properties through the command at 0x1671650 with a pointer
 * to the value; the property lives inside SamplePadControls (root+0x178) and
 * has guards at +0x38/+0x78/+0x80/+0x88. Find the one that references our trim
 * parameter, then set it the same way. */

/* The page draws its locators from the trim **slider widget**: a ui::Slider
 * embedded in the screen holds its position as a percentage at +0x1b4
 * (measured: +0x6758 = 49.0 right after K1 was set to 50). Writing that value
 * and repainting is what a knob move does to the widget. */

/* Apply the trim to one screen instance: find the two trim sliders by the
 * instance's own cached percentages, write ours into their +0x1b4 and repaint.
 * Returns 1 when both were found. */

/* The page paints from the *model*. Find the TrimParameter instances whose
 * value matches the page's own cached percentages (screen+0x1f4c / +0x1f50):
 * those are the parameters the page is actually showing. */

/* Find the page object by its own cached percentage: a float equal to the
 * End percent whose address-0x1f50 starts with a vtable pointer. Works for any
 * screen subclass, which is what the enumeration by one vptr misses. */

/* Enumerate objects of the Trim page class and pick the one whose cached
 * percentages match what the layer currently holds. */

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
    if (!g_in_cc_replay && ev && ((unsigned char *)ev)[8] == 0x13) {
        looper_set_master(raw); /* MAIN VOLUME drives the loops ... */
        if (minimod_now_ms() > g_cfg_saved_at + 5000) {
            g_cfg_saved_at = minimod_now_ms();
            looper_cfg_save();
        }
        /* ... and the app's own mix, so fall through to orig_cc */
    }
    if (!g_in_cc_replay && ev && looper_is_open()) {
        unsigned cc = ((unsigned char *)ev)[8];
        if (cc == 0x10) { /* K1: 1 ms per step, clamped to +-400 ms */
            static int k1_last = -1;
            if (k1_last < 0) {
                k1_last = (int)raw;
            } else if ((int)raw != k1_last) {
                long frames = (long)g_lat_extra +
                              ((long)raw - (long)k1_last) * 44L;
                if (frames < -400L * 44)
                    frames = -400L * 44;
                if (frames > 400L * 44)
                    frames = 400L * 44;
                g_lat_extra = (int)frames;
                if (minimod_now_ms() > g_cfg_saved_at + 5000) {
                    g_cfg_saved_at = minimod_now_ms();
                    looper_cfg_save();
                }
                k1_last = (int)raw;
                char line[40];
                snprintf(line, sizeof line, "TRIM %+d ms", (int)(frames / 44));
                looper_notice(line);
            }
            return;
        }
        if (cc != 0x13)
            return; /* the looper owns the other controls while it is open */
    }
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
 /* optional byte copy for diffs */



/* The three knobs' properties are consecutive objects 0x58 apart (K1 at
 * base+0x1ac0, K2 at +0x1b18, K3 at +0x1b70). Finding that triple identifies
 * the control surface without needing a knob move first. */


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
    g_toast_until = minimod_now_ms() + 1000;
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
    if (g_toast_until > minimod_now_ms() && g_toast_text[0]) {
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
    if (looper_is_open()) {
        looper_pump();
        looper_draw(g);
        return;
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


/* The soft-key captions are not juce::Label (Label::getText is never called),
 * so find the live juce::String that holds "Filter" once, and rename it in
 * place with the app's own String::operator=(const char*). */


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
    if (!g_midi_on && orig_midi_note_event) {
        orig_midi_note_event(app, context, event, source);
        return;
    }
    if (event) {
        /* Decoding lives in Rust: the external-source rule and the
         * note-on/off rules are the part worth testing. */
        struct minimod_note decoded;
        if (!minimod_midi_decode((const unsigned char *)event, (uint64_t)source,
                                 &decoded))
            decoded = (struct minimod_note){0};
        uint32_t type = decoded.kind;
        float velocity = decoded.velocity;
        uint8_t note = (uint8_t)decoded.note;
        int external = decoded.external;
        int note_on = decoded.note_on;
        int note_off = decoded.note_off;
        int row = decoded.row;
        unsigned char *e = (unsigned char *)event;
        mlog("ev ext=%d src=%llx ctx=%llx app=%llx type=%u note=%u vel=%.3f lv=%d\n",
             external, (unsigned long long)source, (unsigned long long)(uintptr_t)context,
             (unsigned long long)(uintptr_t)app, type & 0xffu, note, velocity, g_levels);
        if (!external) {
            char hex[200]; int k = 0;
            for (int i = 0; i < 64 && k < (int)sizeof hex - 4; ++i)
                k += snprintf(hex + k, sizeof hex - k, " %02x", e[i]);
            mlog("padbytes%s\n", hex);
            g_pad_ctx = (uintptr_t)context;
        }
        if (!external && (type & 0xffu) == 3 && velocity > 0.0f) {
            uint16_t mask = 0;
            memcpy(&mask, e + 0x14, 2);
            g_pad_tune_mode = (mask & 1u) ? 0 : 1;
            if (mask & 1u) {
                g_pad_prop_val = e[0x16];
                g_pad_level_byte = e[0x30];
                g_pad_prop_pending = 1;
            }
        }
        if (levels_active() && external && note <= 127 &&
            ((type & 0xffu) == 3 || note_off)) {
            if (note_on || g_midi_replayed[note]) {
                /* Re-send the note as the selected pad's event: clear the
                 * property mask, put the selected pad in event+0x30, then apply
                 * the pitch offset with the app's own setter - the same call
                 * the 16 Levels renderer makes. */
                uint16_t old_mask;
                uint8_t old_prop, old_note;
                memcpy(&old_mask, e + 0x14, 2);
                old_prop = e[0x16];
                old_note = e[0x30];
                uint16_t mask = 0;
                memcpy(e + 0x14, &mask, 2);
                e[0x30] = (unsigned char)g_pad_level_byte;
                /* Use the app's own property setter: this is exactly the path
                 * PLUS/MINUS uses to shift the row by octaves. */
                if (orig_pitch_off)
                    orig_pitch_off(e + 0x14, 0, 0, row);
                if (note_on) {
                    g_midi_replayed[note] = 1;
                    g_midi_chromatic_note = note;
                    g_midi_chromatic_delta = row;
                    __atomic_add_fetch(&g_midi_chromatic_count, 1, __ATOMIC_RELAXED);
                } else {
                    g_midi_replayed[note] = 0;
                }
                mlog("synth ctx=%llx src=7ffffffa prop=%u e30=%u lv=%d\n",
                     (unsigned long long)g_pad_ctx, e[0x16], e[0x30], g_levels);
                if (orig_midi_note_event)
                    orig_midi_note_event(app, (void *)g_pad_ctx, event, 0x7ffffffa);
                memcpy(e + 0x14, &old_mask, 2);
                e[0x16] = old_prop;
                e[0x30] = old_note;
                return;
            }
        }
    }
    if (orig_midi_note_event)
        orig_midi_note_event(app, context, event, source);
}

/* -------------------------------------------------------- probe threads */

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
        int ok = wav_edges(path, fr, off, ch, bytes, format, g_trim_ratio,
                           &first, &last);
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

/* Probe: /run/minimod.slider holds "N,V" - write percent V into the N-th
 * ui::Slider of the painting screen and repaint. Used to identify which slider
 * drives which locator. */

/* Probe: /run/minimod.heap holds "snap" or "diff" for the heap fingerprint. */

/* Probe: find every place in writable memory where the float pair
 * (0.0, value) sits adjacent - the trim model would keep Start/End that way. */

/* Byte-level snapshot of the small writable ranges: a knob move changes the
 * pad's stored trim, and this names it exactly. */
#define BS_MAX (48u << 20)

/* "findv <float>" - every address in the small writable ranges holding exactly
 * this float. Run with two knob values and intersect to find the stored state. */


/* Probe: "who <addr>" - every object that holds a pointer to <addr>, with the
 * holder's offset and whether it looks like an object (vtable in the image). */

/* Probe: /run/minimod.bcast holds "i,v" - broadcast g_bp[i] with value v, so we
 * can find which recorded property drives the page. */

/* Re-arms the software watchpoint from /run/minimod.watch (hex address), so the
 * address can be set after the app has started and picked a new heap. */

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
    uintptr_t found = 0;
    if (!dl_iterate_phdr(find_main, &found)) {
        mlog("minimod: stock app not found; not installing\n");
        return;
    }
    base = found;
    if (!firmware_ok()) {
        mlog("minimod: firmware does not match this build; not installing\n");
        return;
    }

    const char *env = getenv("MINIMOD_HOOKS");
    if (env && *env)
        hook_mask = atoi(env);
    const char *safe_env = getenv("MINIMOD_SAFE");
    const char *dbg_env = getenv("MINIMOD_DEBUG");
    safe_mode = safe_env && atoi(safe_env) != 0;
    debug_mode = dbg_env && atoi(dbg_env) != 0;
    const char *probe_env = getenv("MINIMOD_PROBE");
    probe_mode = probe_env && atoi(probe_env) != 0;
    real_hw_malloc = (unsigned (*)(void **))dlsym(RTLD_NEXT, "snd_pcm_hw_params_malloc");
    real_hw_current = (int (*)(void *, void *))dlsym(RTLD_NEXT, "snd_pcm_hw_params_current");
    real_hw_bufsize = (int (*)(void *, unsigned long *))dlsym(RTLD_NEXT, "snd_pcm_hw_params_get_buffer_size");
    real_hw_channels = (int (*)(void *, unsigned *))dlsym(RTLD_NEXT, "snd_pcm_hw_params_get_channels");
    real_hw_format = (int (*)(void *, int *))dlsym(RTLD_NEXT, "snd_pcm_hw_params_get_format");
    real_hw_free = (void (*)(void *))dlsym(RTLD_NEXT, "snd_pcm_hw_params_free");
    looper_cfg_load();
    {
        const char *lat = getenv("MINIMOD_LOOP_LAT");
        if (lat && *lat)
            g_lat_fixed = atoi(lat);
        const char *extra = getenv("MINIMOD_LOOP_LAT_EXTRA");
        if (extra && *extra)
            g_lat_extra = atoi(extra);
    }
    const char *bits_env = getenv("MINIMOD_PROBES");
    if (bits_env && *bits_env)
        probe_bits = atoi(bits_env);
    void *t_ctor = (hook_mask & 1) ? install_hook(base + A_SIXTEEN_CTOR, (void *)sixteen_ctor) : 0;
    void *t_proc = (hook_mask & 2) ? install_hook(base + A_SIXTEEN_PROC, (void *)sixteen_proc) : 0;
    void *t_sync = install_hook(base + 0x2822260, (void *)sync_probe);
    if (t_sync)
        orig_sync = (void (*)(void *, void *, void *, void *))t_sync;
    void *t_data = install_hook(base + A_DATA_SIGNAL, (void *)data_signal);
    if (midi_hook_site_ok()) {
        void *t_midi = install_hook(base + A_MIDI_NOTE_EVENT, (void *)midi_note_event);
        if (t_midi) {
            orig_midi_note_event =
                (void (*)(void *, void *, void *, uintptr_t))t_midi;
            g_midi_ready = 1;
        }
    } else {
        mlog("minimod: MIDI dispatcher mismatch; not hooking\n");
    }
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
    /* The MIDI dispatcher is hooked lazily by midi_hook_set(): outside
     * 16 Levels the stock code must stay untouched. */
    mlog("minimod: MIDI hook deferred until 16 LEVELS\n");
    void *t_btn = (hook_mask & 16) ? install_hook(base + A_BUTTONS, (void *)buttons) : 0;
    void *t_note = (hook_mask & 32) ? install_hook(base + A_NOTE_ON, (void *)note_on) : 0;
    void *t_cs = (hook_mask & 64) ? install_hook(base + A_CS_EVENT, (void *)cs_event) : 0;
    void *t_off = (hook_mask & 128) ? install_hook(base + A_NOTE_OFF, (void *)note_off) : 0;
    void *t_bass_audio = install_hook(base + A_BASS_AUDIO_WRITER, (void *)audio_writer);
    void *t_sc = install_hook(base + 0x1968020, (void *)setctl_probe);
    if (t_sc)
        orig_setctl = (void (*)(void *, void *, int, int, float))t_sc;
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
    if (!minimod_hooks_touched()) {
        mlog("minimod: no hook installed\n");
        return;
    }
    /* Hardening failures must never take the host app down: the engine leaves
     * the pages writable and the app keeps running. */
    minimod_seal_hooks();
    mlog("minimod: base=%p mask=%d safe=%d probe=%d (ctor=%d proc=%d plus=%d minus=%d)\n",
         (void *)base, hook_mask, safe_mode, probe_mode, t_ctor != 0, t_proc != 0);
}
