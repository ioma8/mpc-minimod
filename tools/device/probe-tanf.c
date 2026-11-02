/* probe-tanf.c -- read the device's filter cutoff mapping off the audio thread.
 *
 * The app builds every JUCE IIR coefficient through tanf(pi * f / fs)
 * (FINDINGS 73), and tanf is an ordinary dynamic import (R_AARCH64_JUMP_SLOT),
 * so an LD_PRELOAD interposer sees the *actual* cutoff in Hz that the app uses
 * for each pad.  That mapping -- normalised filterCutoff to Hz -- is the last
 * filter unknown, and it cannot be recovered from a mixdown (a broad valley of
 * scales fits equally well; FORMAT.md section 10).
 *
 * One line is written per *distinct* cutoff, so the audio thread performs a
 * handful of writes in total and the result does not depend on timing.
 *
 *   MPC_PROBE_LOG  output path (default /data/mpc-probe.log, then /tmp)
 *   MPC_PROBE_FS   engine rate used for the Hz conversion (default 44100)
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#ifndef PROBE_TEST
#include <link.h>
#endif

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

#define MAX_DISTINCT 512
#define HZ_EPS 0.5f

static float (*real_tanf)(float);
static FILE *out;
static float seen[MAX_DISTINCT];
static int nseen;
static double fs = 44100.0;

#ifndef PROBE_TEST
struct probe_phdr {
    uintptr_t base;
    int found;
};

/* The main executable is the image with an empty name. */
static int probe_cb(struct dl_phdr_info *info, size_t size, void *data) {
    struct probe_phdr *st = (struct probe_phdr *)data;
    (void)size;
    if (st->found) return 1;
    if (info->dlpi_name && info->dlpi_name[0] != '\0') return 0;
    st->base = (uintptr_t)info->dlpi_addr;
    st->found = 1;
    return 1;
}

static uintptr_t image_base(void) {
    struct probe_phdr st = {0, 0};
    dl_iterate_phdr(probe_cb, &st);
    return st.base;
}
#else
static uintptr_t image_base(void) { return 0; }
#endif

static void probe_open(void) {
    if (out) return;
    const char *p = getenv("MPC_PROBE_LOG");
    const char *f = getenv("MPC_PROBE_FS");
    if (f && *f) {
        double v = atof(f);
        if (v > 1000.0 && v < 400000.0) fs = v;
    }
    if (p && *p) out = fopen(p, "w");
    if (!out) out = fopen("/data/mpc-probe.log", "w");
    if (!out) out = fopen("/tmp/mpc-probe.log", "w");
    if (!out) return;
    fprintf(out, "# mpc filter probe pid=%ld fs=%.0f base=0x%lx\n",
            (long)getpid(), fs, (unsigned long)image_base());
    fflush(out);
}

/* One line per distinct cutoff.  `lr` is the call site, so lines from the four
 * JUCE coefficient builders (0xfcc988, 0xfcca4c, 0xfccb0c, 0xfccbe0) can be
 * told apart from any other tanf user in the app. */
static void probe_record(float x, uintptr_t lr) {
    if (!(x > 0.0f) || !isfinite((double)x)) return;
    float hz = (float)((double)x * fs / M_PI);
    if (!(hz > 1.0f) || hz > 30000.0f) return;
    for (int i = 0; i < nseen; i++) {
        if (fabsf(seen[i] - hz) <= HZ_EPS) return;
    }
    if (nseen >= MAX_DISTINCT) return;
    seen[nseen++] = hz;
    probe_open();
    if (!out) return;
    fprintf(out, "hz=%.3f lr=0x%lx\n", (double)hz, (unsigned long)lr);
    fflush(out);
}

#ifdef PROBE_TEST
void probe_record_for_test(float x, uintptr_t lr) { probe_record(x, lr); }
void probe_close_for_test(void) {
    if (out) fclose(out);
    out = NULL;
    nseen = 0;
}
#else
float tanf(float x) {
    if (!real_tanf) real_tanf = (float (*)(float))dlsym(RTLD_NEXT, "tanf");
    probe_record(x, (uintptr_t)__builtin_return_address(0));
    if (!real_tanf) return (float)tan((double)x);
    return real_tanf(x);
}

__attribute__((destructor)) static void probe_flush(void) {
    if (out) fclose(out);
    out = NULL;
}
#endif
