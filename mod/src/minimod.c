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

#include "hpss_ffi.h"
#include "h3m_effect.h"
#include "job_ffi.h"
#include "spectral_machine_ffi.h"
#include "routing_ffi.h"
#include "audio_ffi.h"
#include "audio_ffi.h"
#include "substitute_ffi.h"
#include "hooks_ffi.h"
#include "levels_ffi.h"
#include "hooks_ffi.h"
#include "page_ffi.h"
#include "selection_ffi.h"
#include "chop_ffi.h"
#include "pads_ffi.h"
#include "cfg_ffi.h"
#include "autotrim_ffi.h"
#include "buttons_ffi.h"
#include "hpss_exec_ffi.h"
#include "props_ffi.h"

#include "ui_ffi.h"

/* The memory window is Rust now; the call sites did not change. */
int rd(uintptr_t address, void *dst, size_t length);
int mapped(const void *address);

/* The page the mod last painted lives in Rust, with the rest of the paint state. */
uintptr_t *minimod_screen_slot(void);
#define g_screen (*(void **)minimod_screen_slot())
#include "entries_ffi.h"
#include "glue_ffi.h"

struct minimod_ui_state *minimod_ui_state(void);
#define g_shift (minimod_ui_state()->shift)
#define g_erase_down (minimod_ui_state()->erase)
#define g_trim_step (minimod_ui_state()->trim_step)
#define g_b3_down (minimod_ui_state()->b3_down)
#define g_plus_held (minimod_ui_state()->plus_held)
#define g_minus_held (minimod_ui_state()->minus_held)
#define g_octave (minimod_ui_state()->octave)
#define g_mark_v0 (minimod_ui_state()->mark_v0)
#define g_mark_v1 (minimod_ui_state()->mark_v1)
#define g_last_press_obj (minimod_ui_state()->last_press_obj)
#define g_hpss_btn (minimod_ui_state()->event)
#define g_hpss_btn_valid (minimod_ui_state()->event_valid)

#include "cc_ffi.h"
#include "spectral_ffi.h"
#include "menu_ffi.h"
#include "overlay_ffi.h"
#include "paint_ffi.h"
#include "menu_ffi.h"
#include "input_ffi.h"
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


static void mlog(const char *fmt, ...);
static void toast(const char *text);
#include "clock_ffi.h"
static struct minimod_menu g_menu __attribute__((used));
#define g_hpss_menu g_menu.open
#define g_hpss_busy g_menu.busy
#define g_bass_edit g_menu.bass_edit
#define g_hpss_data_down g_menu.data_down
#define g_hpss_restore_pending g_menu.restore_pending
struct minimod_input *minimod_input_state(void);
#define g_input (*minimod_input_state())
static void input_context(struct minimod_input_context *ctx);
static void looper_pump(void);

#define MAXW 12






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
static pthread_mutex_t g_mem_mtx = PTHREAD_MUTEX_INITIALIZER;





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
static int g_midi_ready;
/* A physical pad press identifies its pad through property 0 of the note event
 * (event+0x16), whose value is the bank's pad-note base plus the pad index.
 * Learn that base from a real pad press so MIDI can address the same row. */
static struct minimod_midi_state g_midi;

static int midi_hook_site_ok(void)
{
    return minimod_midi_site_ok(base);
}

static void midi_hook_set(int on)
{
    /* The dispatcher is hooked once at load; this only changes behaviour, so no
     * code is rewritten while the MIDI thread may be running it. */
    g_midi.on = on ? 1 : 0;
}

/* -------------------------------------------------------- 16 Levels state */
/* 16 LEVELS mode lives in `g_midi.levels`, tracked from button 0x28. */
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
        if (g_midi.levels) mlog("lv: predicate 0, clearing\n");
        g_midi.levels = 0;
        midi_hook_set(0);
        return 0;
    }
    return g_midi.levels;
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












/* The page binds each trim parameter through a small binding object whose
 * vtable is app+0x6e08a10 and which keeps the property pointer at +0x20. */
#define A_BINDING_VT 0x6e08a10







/* A property seen in a real broadcast whose cached value is exactly the
 * parameter's current value is the one that owns it. Exact equality keeps the
 * false positives (and the abort they caused) away. */

/* ------------------------------------------------------------ app screens */




/* ------------------------------------------------------------- app hooks */
static void *(*orig_sixteen_ctor)(void *, void *, void *, void *);
static void (*orig_sixteen_proc)(void *, int, void *);
static void (*orig_pitch_off)(void *, uint64_t, uint64_t, int);
static void (*orig_buttons)(void *);
static void (*orig_midi_note_event)(void *, void *, void *, uintptr_t);
static void (*orig_bass_audio_writer)(void *, void *, size_t, int);
static volatile unsigned g_audio_in_calls;
static volatile unsigned g_audio_out_calls;
/* The Sample page (0x2468 bytes) captured from SamplePage_Ctor; it owns the
 * Start/End widgets and is what must be repainted after a property change. */
typedef void (*pagector_fn)(void *, void *, void *, void *, void *, void *,
                            void *, void *, void *, void *, void *, void *);


/* The app's own "apply a control value" entry - exactly what a knob turn calls
 * (FUN_01a68020(0, surface, controlId, value, mode)). Capturing the surface lets
 * commit_trim drive the Start/End controls the same way the UI does. */
static void *g_surface;
static void *g_control_context;
static void (*orig_setctl)(void *, void *, int, int, float);


/* The page object, captured from the control handler FUN_02904e90(page, id, ...)
 * - refreshed on every knob turn, so it is always a live pointer. */


/* FUN_02912f10(padBlock) returns the object the page reads its trim from
 * (Start at +0x78, End at +0xd0). Capturing it in-process avoids stale pointers. */


/* PadState_CopyIntoPage(page, padState): the pad is the source of the page's
 * Start/End widgets (pad+0x1588 -> widget value, pad+0x15e0 -> End). */




/* AC50WaveformLocator::bounds (0x16b6060): x = pos[+0x168] * view->w[+0x28] - 2.
 * Hooking it records the locators that are actually painting, which is the only
 * way to know which objects the page positions. */
#define MAX_LOC 12

static void (*orig_str_assign)(void *, const char *);
static void *(*orig_label_text)(void *);
static void (*orig_draw_text)(void *, const uint64_t *, int, int, int, int, int, int, float);
static void (*orig_repaint)(void *, uint64_t, uint64_t);

static void set_filter_text(const char *t);
static void do_autotrim(void);


static volatile int g_buf_dirty; /* re-read the device rings on the audio thread */


/* The stock code computes the 16 Levels pitch here; shift the whole row. */


/* "findv <float>" - every address in the small writable ranges holding exactly
 * this float. Run with two knob values and intersect to find the stored state. */

static void (*orig_screen_paint)(void *, void *);


/* Write the property values immediately before the app paints, so the paint
 * reads the trim we computed. */

static void *g_paint_comp;
static void *g_paint_gfx;

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
static volatile int g_hpss_write_ok;
static void *g_hpss_cmd[4];
static int g_hpss_cmd_n;

static uint64_t g_hpss_name[6];
static int g_hpss_names_ready;
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
static void hpss_repaint(void);


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
    struct minimod_hpss_arm arm = {
        .armed = armed && g_hpss_arm,
        .buffer = (uintptr_t)g_hpss_arm_buf,
        .frames = g_hpss_arm_frames,
        .source_frames = g_hpss_arm_source_frames,
        .channels = g_hpss_arm_ch,
        .start = g_hpss_arm_start,
        .end = g_hpss_arm_end,
        .rename = g_hpss_arm_name,
    };
    struct minimod_hpss_outcome outcome =
        minimod_hpss_exec_run((uintptr_t)cmd, &arm, (void (*)(void *))orig_hpss_exec);
    if (arm.armed)
        g_hpss_write_ok = outcome.write_ok;
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
/* The layout is defined once, in rust/crates/minimod/src/job.rs. */
typedef struct minimod_job hpss_job;

static volatile int g_hpss_done;
static int g_hpss_chop_before;
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



static void hpss_finished(void)
{
    __atomic_store_n(&g_hpss_done, 1, __ATOMIC_RELEASE);
}

static const struct minimod_glue_hooks g_job_hooks;

static void sample_transform_start(int action)
{
    minimod_glue_start(action, base, (uintptr_t)g_screen, g_bass_knob, &g_menu,
                       &g_job_hooks);
}

static void hpss_start(void) { sample_transform_start(0); }
static void effect_start(int action) { sample_transform_start(action); }





static void fmbass_start(void)
{
    minimod_glue_start(4, base, (uintptr_t)g_screen, g_bass_knob, &g_menu, &g_job_hooks);
}

static void hpss_close(void);

/* ------------------------------------------------------- spectral markers */
/* The input hook is NOT the UI thread. Publish only a request there; all
 * model calls below run from screen_paint, after the stock paint returns. */
static struct minimod_spectral_state g_spectral;




/* Same binding path used by the stock Split handler (Ghidra 0x1756728).
 * Manual is enum 0. Calling the binding's model callback preserves the pad's
 * chop type; assigning the view's cached enum alone does not. */



static void spectral_start(void)
{
    if (minimod_spectral_start(&g_spectral, g_hpss_btn, (int32_t)sizeof g_hpss_btn,
                               g_hpss_busy || minimod_job_busy()))
        hpss_repaint();
}

/* One phase per UI pass: a CHOP press schedules a page switch, so it cannot
 * be followed by a sleep/poll or by another toggle in the same callback. */
/* The three-phase dance (open Manual chop, write the boundaries, verify them)
 * lives in Rust; the glue keeps the sample read and applies each frame's result. */




/* The machine presses CHOP through the app's own button handler; the event it
 * replays is the one that started the run, which lives in the Rust state. */
static void spectral_press_chop_now(void)
{
    if (!orig_buttons)
        return;
    g_spectral.event[8] = 0x4b;
    g_spectral.event[9] = 0x7f;
    orig_buttons(g_spectral.event);
    g_spectral.event[9] = 0;
    orig_buttons(g_spectral.event);
}

static void spectral_tick(void)
{
    struct minimod_spectral_hooks hooks = {0};
    hooks.press_chop = spectral_press_chop_now;
    hooks.set_status = hpss_set_status;
    hooks.repaint = hpss_repaint;
    minimod_spectral_tick(&g_spectral, &g_menu, base, (uintptr_t)g_screen, &hooks);
}


static int hpss_extract_hook(uintptr_t screen, uintptr_t state, const float *buf,
                             unsigned start, unsigned end, unsigned source_frames,
                             unsigned buffer_frames, unsigned ch, int name, int *dest)
{
    return hpss_extract_ctx((void *)screen, state, buf, start, end, source_frames,
                            buffer_frames, ch, name, dest);
}

/* The checks, the extracts and the words live in Rust; the glue supplies the
 * app's extract, the chop restore and the toast. */


static void bass_preview_off(void)
{
    bass_preview_set_active(0);
}

static const struct minimod_glue_hooks g_job_hooks = {
    .finished = hpss_finished,
    .repaint = hpss_repaint,
    .toast = toast,
    .chop_mode = hpss_chop_mode,
    .restore_chop = hpss_restore_chop,
    .preview_off = bass_preview_off,
    .extract = (void (*)(uintptr_t, uintptr_t, const float *, uint32_t, uint32_t,
                         uint32_t, uint32_t, uint32_t, int32_t, int32_t *))hpss_extract_hook,
};

static void hpss_finalize(void)
{
    minimod_glue_finish(base, (uintptr_t)g_screen, &g_menu, &g_job_hooks);
}

static void hpss_open(void)
{
    if (!sample_page_visible() || g_hpss_busy)
        return;
    g_hpss_menu = 1;
    g_menu.selection = 0;
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

static void bass_page_open(void)
{
    g_menu.bass_edit = 1;
    bass_preview_set_params(g_bass_knob[0], g_bass_knob[1], g_bass_knob[2]);
    bass_preview_set_active(1);
    hpss_set_status(g_bass_preview_available ? "" : "Live preview unavailable");
}

static int looper_menu_open(void)
{
    int opened = looper_activate();
    if (opened) {
        g_buf_dirty = 1; /* re-read the device rings on its own thread */
        bass_preview_set_active(0);
        looper_pump();
    }
    return opened;
}

static void hpss_activate(void)
{
    struct minimod_menu_hooks hooks = {0};
    hooks.repaint = hpss_repaint;
    hooks.start_split = hpss_start;
    hooks.start_effect = effect_start;
    hooks.start_spectral = spectral_start;
    hooks.start_bass = fmbass_start;
    hooks.start_bass_page = bass_page_open;
    hooks.start_looper = looper_menu_open;
    hooks.set_status = hpss_set_status;
    minimod_menu_activate(&g_menu, &hooks);
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




static void hpss_draw_menu(void *g)
{
    minimod_menu_draw((uintptr_t)g, g_menu.selection, g_menu.bass_edit, g_menu.busy,
                      g_hpss_status, g_bass_knob);
}

static void release_held_pads(void);


static void ui_forward(void *event)
{
    if (orig_buttons)
        orig_buttons(event);
}

static void ui_clear_marks(void)
{
    if (g_screen)
        FN(0x1149880, void (*)(void *, uint64_t, uint64_t))(
            g_screen, 0, FIELD(g_screen, 0x28, uint64_t));
}

static uintptr_t ui_read_pointer(void *event)
{
    uintptr_t value = 0;
    if (event && rd((uintptr_t)event, &value, sizeof value))
        return value;
    return 0;
}

static int32_t ui_bump_octave(int32_t steps)
{
    return bump_octave((int)steps);
}

static void ui_release_held(void)
{
    release_held_pads();
}

static void ui_toggle_levels(void)
{
    g_midi.levels = !g_midi.levels;
    if (g_midi.levels && g_input.last_pad >= 0 && g_input.last_pad < 16)
        g_midi.pad_level_byte = g_input.last_pad;
    midi_hook_set(levels_active());
}

static int32_t ui_menu_open(void) { return g_hpss_menu ? 1 : 0; }
static int32_t ui_bass_edit(void) { return g_bass_edit ? 1 : 0; }
static int32_t ui_busy(void) { return g_hpss_busy ? 1 : 0; }
static int32_t ui_spectral(void) { return g_spectral.active == 1 ? 1 : 0; }
static int32_t ui_page_visible(void)
{
    return sample_page_visible();
}

static void ui_menu_open_hook(void)
{
    hpss_open();
}

static const struct minimod_ui_hooks g_ui_hooks = {
    .forward = ui_forward,
    .toggle_levels = ui_toggle_levels,
    .menu_open = ui_menu_open_hook,
    .menu_close = hpss_close,
    .menu_activate = hpss_activate,
    .filter_text = set_filter_text,
    .midi_sync = midi_hook_set,
    .levels_active = levels_active,
    .page_visible = ui_page_visible,
    .restore_chop = hpss_restore_chop,
    .repaint = hpss_repaint,
    .clear_marks = ui_clear_marks,
    .bump_octave = ui_bump_octave,
    .run_autotrim = do_autotrim,
    .release_held = ui_release_held,
    .read_pointer = ui_read_pointer,
    .menu_open_state = ui_menu_open,
    .bass_edit_state = ui_bass_edit,
    .busy_state = ui_busy,
    .spectral_state = ui_spectral,
};

static void ui_buttons(void *event)
{
    minimod_hook_buttons(event, &g_ui_hooks);
}

/* The Rust entry points ask for this once per event. */
struct minimod_input_context minimod_glue_context(void);
int *minimod_glue_selection(void);

struct minimod_input_context minimod_glue_context(void)
{
    struct minimod_input_context ctx;
    input_context(&ctx);
    return ctx;
}

int *minimod_glue_selection(void)
{
    return &g_menu.selection;
}

static void input_context(struct minimod_input_context *ctx)
{
    ctx->looper_open = looper_is_open();
    ctx->spectral_active = g_spectral.active;
    ctx->preview_active = __atomic_load_n(&g_bass_preview_active, __ATOMIC_ACQUIRE);
    ctx->menu_open = g_hpss_menu;
    ctx->bass_edit = g_bass_edit;
    ctx->busy = g_hpss_busy;
    ctx->shift = g_shift;
    ctx->erase = g_erase_down;
    ctx->repaint = hpss_repaint;
}

static void release_held_pads(void)
{
    minimod_input_release_held(&g_input, (minimod_note_off_fn)minimod_hook_note_off);
}



/* The firmware's audio writer accepts planar float channels, then converts
   them to S32 PCM. Mix the preview and the live-looper loops into private
   block storage so the live engine buffers are never changed. The normal
   device block is 128 frames; the lab runs a larger one, so the copy allows
   up to 4096 and falls through for anything bigger. */

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
    int preview = __atomic_load_n(&g_bass_preview_active, __ATOMIC_ACQUIRE) ? 1 : 0;
    float peak = 0.f;
    if (minimod_audio_writer((uintptr_t)self, (float **)input, frames, offset,
                             (minimod_audio_writer_fn)orig_bass_audio_writer,
                             preview, &peak) &&
        peak > 0.f) {
        __atomic_add_fetch(&g_bass_preview_blocks, 1, __ATOMIC_RELAXED);
        uint32_t previous_bits = __atomic_load_n(&g_bass_preview_peak_bits, __ATOMIC_RELAXED);
        float previous;
        memcpy(&previous, &previous_bits, sizeof previous);
        if (peak > previous)
            __atomic_store_n(&g_bass_preview_peak_bits, bass_float_bits(peak), __ATOMIC_RELAXED);
    }
}








/* The trim core is called by the broadcaster's listener loop, which keeps the
 * property in x20: learning that pair is what lets autotrim drive the page. */

uintptr_t slider_for_value(float pct)
{
    return minimod_slider_for_value((uintptr_t)g_screen, base, pct);
}





/* ------------------------------------------------------- message-thread pump */

/* Broadcast a unique value to one candidate property: if the app's own
 * listener chain reaches the trim core for *our* parameter, that property owns
 * it. The original value is restored right after. */

/* The page keeps a small binding object per parameter (vtable app+0x6e08a10)
 * that holds the property at +0x20. Collect those as probe candidates. */

/* Read-only ownership test: the property's listener chain must reference the
 * parameter (or its model) somewhere within two hops. No broadcasting, so a
 * wrong candidate can never disturb the app. */
int prop_references(uintptr_t P, uintptr_t param)
{
    return minimod_prop_references(P, param);
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

/* Recover a parameter's owning property without watching a knob.
 *
 * A property P keeps its list head at P+0x10 and its sentinel at P+8. A node
 * keeps its target (tagged) at +0x18 and its next at +8; the last next in the
 * chain is the sentinel, so P = sentinel - 8. Validation: P+0x10 must be a
 * mapped node and P+0x50 a plausible float. */

/* The app's own CC path: 0x159f230(range, event, raw) divides raw by range and
 * broadcasts through the property at *(event[0]+0x10) + 0x1ac0 (CC 0x10 = K1)
 * or +0x1b18 (CC 0x11 = K2). Calling it is exactly a knob turn. */
uintptr_t ctl_obj_for(uintptr_t surface)
{
    return minimod_ctl_obj_for(surface);
}

/* A real CC event captured from the app, replayed with our own number so the
 * app performs exactly what a knob does. */
static int g_in_cc_replay;

struct minimod_cc_context minimod_glue_cc_context(void)
{
    struct minimod_cc_context context = {
        .replaying = g_in_cc_replay ? 1 : 0,
        .value = 0,
        .range = 0.0f,
        .looper_open = looper_is_open() ? 1 : 0,
        .menu_open = g_hpss_menu ? 1 : 0,
        .bass_edit = g_bass_edit ? 1 : 0,
        .busy = g_hpss_busy ? 1 : 0,
        .spectral_active = g_spectral.active == 1 ? 1 : 0,
        .trim_frames = g_lat_extra,
        .knobs = {g_bass_knob[0], g_bass_knob[1], g_bass_knob[2]},
    };
    return context;
}

void minimod_glue_set_trim(int32_t frames)
{
    g_lat_extra = (int)frames;
}

void minimod_glue_set_knobs(const float *knobs)
{
    g_bass_knob[0] = knobs[0];
    g_bass_knob[1] = knobs[1];
    g_bass_knob[2] = knobs[2];
    bass_preview_set_params(g_bass_knob[0], g_bass_knob[1], g_bass_knob[2]);
}

void minimod_glue_cc_notice(const char *text)
{
    looper_notice(text);
}

void minimod_glue_repaint(void)
{
    hpss_repaint();
}

void minimod_glue_save(void)
{
    looper_cfg_save();
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


uintptr_t find_knob_props(void)
{
    return minimod_find_knob_props(base);
}

/* Every listener of a real property has a vtable inside the app's
 * .data.rel.ro. If one does not, the property is not the one the page uses and
 * broadcasting through it would jump into garbage. */

/* The trim parameter is the listener of the knob's property whose vtable is
 * TrimParameter's. That makes the Start/End pair exact, with no scanning and no
 * dependence on which knob was touched last. */





/* --------------------------------------------------------------- overlay */
/* Overdraw, the way am-mod does it: take the screen's vtable slot 27 (paint),
 * keep the stock function, and paint on top of it with the app's own Graphics
 * calls (setColour / fillRect / Font + drawText). */
/* The Trim page is its own class (found by matching its cached percentages);
 * it is not the plain SampleModeScreen. */
#define A_TRIM_PAGE_VT 0x6e31ee8

static void (*orig_screen_paint)(void *, void *);

/* The toast's text and deadline live in Rust; the paint hook asks for them. */
static void toast(const char *text)
{
    void *comp = g_paint_comp ? g_paint_comp : g_screen;
    minimod_toast((const unsigned char *)text, (minimod_repaint_fn)orig_repaint,
                  (uintptr_t)comp);
}

__attribute__((unused)) static void draw_colour(void *g, uint32_t v)
{
    minimod_paint_colour((uintptr_t)g, v);
}

__attribute__((unused)) static void draw_rect(void *g, int x, int y, int w, int h)
{
    minimod_paint_rect((uintptr_t)g, x, y, w, h);
}

__attribute__((unused)) static void draw_text(void *g, const char *s, int x, int y, int w, int h,
                      float size)
{
    minimod_paint_text((uintptr_t)g, s, x, y, w, h, size);
}


static void screen_paint(void *p, void *g)
{
    g_paint_comp = p;
    g_paint_gfx = g;
    struct minimod_overlay_hooks hooks = {0};
    hooks.original = (void (*)(void *, void *))orig_screen_paint;
    hooks.spectral_tick = spectral_tick;
    hooks.finalize = hpss_finalize;
    hooks.restore_chop = hpss_restore_chop;
    hooks.looper_pump = looper_pump;
    hooks.menu_draw = (void (*)(void *))hpss_draw_menu;
    struct minimod_overlay_flags flags = {0, g_hpss_restore_pending};
    float marks[2] = {g_mark_v0, g_mark_v1};
    minimod_overlay_paint((uintptr_t)p, (uintptr_t)g, base, &hooks, &flags,
                          (uintptr_t *)&g_screen,
                          g_shift, in_chop_mode(), looper_is_open(), g_hpss_menu,
                          marks);
    g_hpss_restore_pending = flags.restore_pending;
}

static int install_screen_paint(void)
{
    if (!minimod_paint_install(base, (const void *)screen_paint))
        return 0;
    orig_screen_paint = minimod_paint_original();
    return orig_screen_paint ? 1 : 0;
}

/* The soft-key row is rebuilt on SHIFT, and every rebuild assigns the label
 * strings through 0xf535c0(dest, text). While SHIFT is held we swap the B3
 * label ("Filter") for "AutoTrim" and remember its destination so the release
 * can put it back. */

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




/* ------------------------------------------------------------ MIDI (feature 2)
 * Keep the firmware's MIDI input, note-on/off matching, sustain and audio
 * routing intact. The SampQOL reference confirms the native pitch setter takes
 * the event property block at +0x14 and a semitone delta as argument four. */
static void midi_note_event(void *app, void *context, void *event,
                            uintptr_t source)
{
    minimod_midi_handle((uintptr_t)app, (uintptr_t)context, (unsigned char *)event,
                        (uintptr_t)source, &g_midi,
                        (minimod_note_event_fn)orig_midi_note_event,
                        (minimod_pitch_off_fn)orig_pitch_off);
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
    struct minimod_autotrim_hooks hooks = {0};
    hooks.set_control = (void (*)(void *, void *, int32_t, int32_t, float))orig_setctl;
    hooks.repaint = (void (*)(void *, uint64_t, uint64_t))orig_repaint;
    struct minimod_autotrim_context context = {
        .surface = (uintptr_t)g_surface,
        .control = (uintptr_t)g_control_context,
        .component = (uintptr_t)(g_paint_comp ? g_paint_comp : g_screen),
        .step = g_trim_step,
        .last_pad = g_input.last_pad,
    };
    minimod_autotrim_run(&context, &hooks);
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


__attribute__((constructor)) static void load(void)
{
#if MINIMOD_LOG
    log_fd = open("/run/minimod.log", O_WRONLY | O_CREAT | O_APPEND, 0644);
#endif
    uintptr_t found = minimod_find_main();
    if (!found) {
        mlog("minimod: stock app not found; not installing\n");
        return;
    }
    base = found;
    minimod_set_base((uintptr_t)base);
    if (!minimod_firmware_ok(base)) {
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
    void *t_ctor = (hook_mask & 1) ? install_hook(base + A_SIXTEEN_CTOR, (void *)minimod_hook_sixteen_ctor) : 0;
    void *t_proc = (hook_mask & 2) ? install_hook(base + A_SIXTEEN_PROC, (void *)minimod_hook_sixteen_proc) : 0;
    /* The four input hooks are Rust entry points; they are handed the app's own
     * functions once, before any event can arrive. */
    void *t_data = install_hook(base + A_DATA_SIGNAL, (void *)minimod_hook_data);
    void *t_note = (hook_mask & 32) ? install_hook(base + A_NOTE_ON, (void *)minimod_hook_note_on) : 0;
    void *t_cs = (hook_mask & 64) ? install_hook(base + A_CS_EVENT, (void *)minimod_hook_surface) : 0;
    void *t_off = (hook_mask & 128) ? install_hook(base + A_NOTE_OFF, (void *)minimod_hook_note_off) : 0;
    minimod_hook_originals((void *)t_note, (void *)t_off, (void *)t_cs, (void *)t_data);
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
    void *t_cc = install_hook(base + 0x159f230, (void *)minimod_hook_cc);
    minimod_hook_cc_original((void (*)(void *, uint32_t, float, float))t_cc);
    void *t_poff = install_hook(base + A_PITCH_OFF, (void *)minimod_hook_pitch_off);
    if (t_poff)
        orig_pitch_off = (void (*)(void *, uint64_t, uint64_t, int))t_poff;
    /* The MIDI dispatcher is hooked lazily by midi_hook_set(): outside
     * 16 Levels the stock code must stay untouched. */
    mlog("minimod: MIDI hook deferred until 16 LEVELS\n");
    void *t_btn = (hook_mask & 16) ? install_hook(base + A_BUTTONS, (void *)ui_buttons) : 0;
    void *t_bass_audio = install_hook(base + A_BASS_AUDIO_WRITER, (void *)audio_writer);
    void *t_rp = install_hook(base + A_REPAINT, (void *)minimod_hook_repaint);
    if (t_rp)
        orig_repaint = (void (*)(void *, uint64_t, uint64_t))t_rp;
    void *t_sa = install_hook(base + A_STR_ASSIGN, (void *)minimod_hook_str_assign);

    void *t_lt = 0;   /* Label::getText hook removed: wrong ABI, crashed the sample browser */
    void *t_dt = 0;   /* drawText overlay removed: the third slot is never drawn through it */
    if (t_ctor)
        orig_sixteen_ctor = (void *(*)(void *, void *, void *, void *))t_ctor;
    if (t_proc)
        orig_sixteen_proc = (void (*)(void *, int, void *))t_proc;
    if (t_btn)
        orig_buttons = (void (*)(void *))t_btn;
    if (t_bass_audio) {
        orig_bass_audio_writer = (void (*)(void *, void *, size_t, int))t_bass_audio;
    }
    g_bass_preview_available = t_note && t_off && t_bass_audio;
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
        t = install_hook(base + A_HPSS_WRITE_CB, (void *)minimod_hook_hpss_writer);
        minimod_entries_originals((void *)t_ctor, (void *)t_proc, (void *)t_poff,
                                  (void *)t_rp, (void *)t_sa, (void *)t);
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
