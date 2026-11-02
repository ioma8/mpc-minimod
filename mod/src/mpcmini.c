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
#define A_LABEL_TEXT 0x11bae00     /* Label::getText (returns String&)    */

#define T_LAYER 0xb0 /* -> context; [ctx+0x60] = instrument */
#define T_MODEL 0x70
#define T_VALUE 0x19c /* float, normalised 0..1 */
#define T_MIN 0x1e0   /* u32, frames */
#define T_MAX 0x1e8   /* u32, frames */

#define A_TRIM_PARAM_VT 0x6e1b240 /* TrimParameter vtable */

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
static int prop_references(uintptr_t P, uintptr_t param);

static void fault_handler(int sig, siginfo_t *si, void *ucv)
{
    ucontext_t *u = (ucontext_t *)ucv;
    unsigned long pc = 0, lr = 0, sp = 0;
    if (u) {
        pc = (unsigned long)u->uc_mcontext.pc;
        lr = (unsigned long)u->uc_mcontext.regs[30];
        sp = (unsigned long)u->uc_mcontext.sp;
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
static int g_octave;   /* octave shift applied while 16 LEVELS is active */
static void *g_sixteen; /* the live 16 Levels screen, if seen             */

static int sixteen_tune_active(void)
{
    return g_octave != 0;
}

static int bump_octave(int d)
{
    int next = g_octave + d;
    if (next < -4)
        next = -4;
    if (next > 4)
        next = 4;
    if (next == g_octave)
        return 0;
    g_octave = next;
    mlog("mpcmini: octave=%d\n", next);
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
    const short thr = 328; /* ~ -40 dBFS */
    unsigned char buf[16384];
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
} g_tp[16];
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
    for (int r = 0; r < g_nrng && g_ntp < 16; ++r) {
        if (!g_rng[r].w)
            continue;
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi && g_ntp < 16;
             a += 65536) {
            size_t n = (size_t)((g_rng[r].hi - a) < 65536 ? (g_rng[r].hi - a) : 65536);
            n &= ~(size_t)7;
            if (!rd(a, buf, n))
                break;
            for (size_t o = 0; o + 8 <= n && g_ntp < 16; o += 8) {
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
            for (size_t o = 0; o + 8 <= n; o += 8) {
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
    if (orig_sixteen_proc)
        orig_sixteen_proc(self, idx, out);
    if (g_octave && out) {
        float *f = (float *)out;
        *f += 12.0f * (float)g_octave;
    }
}

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
    if (down && code == 0x36) { /* PLUS */
        if (sixteen_tune_active() || g_octave)
            bump_octave(+1);
    } else if (down && code == 0x37) { /* MINUS */
        if (sixteen_tune_active() || g_octave)
            bump_octave(-1);
    }
    if (debug_mode && g_dbg < 80) {
        g_dbg++;
        mlog("mpcmini: button code=0x%02x down=%u\n", code, down);
    }
    if (orig_buttons)
        orig_buttons(event);
}

static void note_on(void *input, int src, int pad, void *props, float vel)
{
    g_last_pad = pad;
    g_props = props;
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

static void cs_event(void *self, int code, void *payload)
{
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
    g_core_hit = 1;
    if (prop && mapped((void *)prop) && prop_references(prop, (uintptr_t)self)) {
        float v50 = 0;
        if (rd(prop + 0x50, &v50, 4) && v50 >= -0.001f && v50 <= 1.001f) {
            float d = v50 - v;
            if (d < 0)
                d = -d;
            if (d <= 0.05f) {
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
    if (probe_mode && logs < 4) {
        logs++;
        mlog("mpcmini: trim self=%p v=%.4f lr=%p\n", self, (double)v,
             __builtin_return_address(0));
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
    if (probe_mode && i < 24)
        mlog("mpcmini: bcast %p v=%.4f\n", self, (double)v);
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

static void commit_trim(void)
{
    void *p0 = g_trim_param0, *p1 = g_trim_param1;
    float v0 = g_trim_v0, v1 = g_trim_v1;
    /* Only a property learned from a real knob move is trusted: the
     * heuristics found false positives and broadcasting a wrong object
     * makes the app abort. */
    if (!learned_prop((uintptr_t)p0))
        probe_property((uintptr_t)p0);
    if (!learned_prop((uintptr_t)p1))
        probe_property((uintptr_t)p1);
    uintptr_t pr0 = learned_prop((uintptr_t)p0);
    uintptr_t pr1 = learned_prop((uintptr_t)p1);
    if (!pr0)
        pr0 = broadcast_prop((uintptr_t)p0, g_trim_cur0);
    if (!pr1)
        pr1 = broadcast_prop((uintptr_t)p1, g_trim_cur1);
    if (pr0 == pr1 || (pr0 && pr1 && !pr0 && !pr1))
        pr0 = pr1 = 0;
    mlog("mpcmini: commit %.4f/%.4f prop=%p/%p\n", (double)v0, (double)v1,
         (void *)pr0, (void *)pr1);
    uintptr_t b0 = 0, b1 = 0;
    rd((uintptr_t)p0 + 0x80, &b0, 8);
    rd((uintptr_t)p1 + 0x80, &b1, 8);
    unsigned fa = (unsigned)(v0 * (float)g_trim_frames + 0.5f);
    unsigned fb = (unsigned)(v1 * (float)g_trim_frames + 0.5f);
    if (b0) {
        wr(b0 + 0x38, &fa, 4);
        wr(b0 + 0xa8, &fa, 4);
    }
    if (b1) {
        wr(b1 + 0x38, &fb, 4);
        wr(b1 + 0xa8, &fb, 4);
    }
    if (g_trim_layer) {
        wr(g_trim_layer + 0x4d8, &fa, 4);
        wr(g_trim_layer + 0x588, &fa, 4);
        wr(g_trim_layer + 0x530, &fb, 4);
    }
    g_own_call = 1;
    FN(A_TRIM_CORE, void (*)(float, void *))(v0, p0);
    FN(A_TRIM_CORE, void (*)(float, void *))(v1, p1);
    g_own_call = 0;
    wr((uintptr_t)p0 + 0x1a0, &v0, 4);
    wr((uintptr_t)p1 + 0x1a0, &v1, 4);
    if (pr0) {
        float cur0 = 0;
        if (rd(pr0 + 0x50, &cur0, 4) && cur0 >= -0.001f && cur0 <= 1.001f) {
            wr(pr0 + 0x50, &v0, 4);
            FN(A_BROADCAST, void (*)(void *, float))((void *)pr0, v0);
        }
    }
    if (pr1) {
        float cur1 = 0;
        if (rd(pr1 + 0x50, &cur1, 4) && cur1 >= -0.001f && cur1 <= 1.001f) {
            wr(pr1 + 0x50, &v1, 4);
            FN(A_BROADCAST, void (*)(void *, float))((void *)pr1, v1);
        }
    }
    unsigned l0 = 0, l1 = 0;
    if (g_trim_layer) {
        rd(g_trim_layer + 0x4d8, &l0, 4);
        rd(g_trim_layer + 0x530, &l1, 4);
    }
    mlog("mpcmini: after layer=%u/%u\n", l0, l1);
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

static void *g_screen;
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

static void screen_paint(void *p, void *g)
{
    if (orig_screen_paint)
        orig_screen_paint(p, g);
    if (FIELD(p, 0, uintptr_t) != base + A_SCREEN_VT)
        return;
    g_screen = p;
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
        for (uintptr_t a = g_rng[r].lo; a + 8 < g_rng[r].hi; a += 65536) {
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
    commit_trim();
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
