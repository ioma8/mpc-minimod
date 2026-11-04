"""Compile the production HPSS module on the host and check the invariants.

The device adds the WAV reader, worker and native pad assignment; this test
covers the DSP itself: channel independence, exact reconstruction, silence,
short input and invalid arguments.
"""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "src/hpss.c"
HARNESS = r'''
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "hpss.h"

static int finite_all(const float *p, size_t n)
{
    for (size_t i = 0; i < n; ++i)
        if (!isfinite(p[i]))
            return 0;
    return 1;
}

static double max_err(const float *a, const float *b, size_t n)
{
    double e = 0;
    for (size_t i = 0; i < n; ++i) {
        double d = fabs((double)a[i] - (double)b[i]);
        if (d > e)
            e = d;
    }
    return e;
}

static int run_case(unsigned ch, size_t frames, int silent)
{
    size_t n = frames * ch;
    float *in = calloc(n, sizeof *in);
    float *h = NULL, *p = NULL;
    if (!in)
        return 1;
    if (!silent) {
        for (size_t i = 0; i < frames; ++i)
            for (unsigned c = 0; c < ch; ++c) {
                float v = 0.3f * sinf(2.f * 3.14159265f * 440.f * (float)i / 44100.f);
                if (c == 1 && i % 2000 == 0)
                    v += 0.5f;
                in[i * ch + c] = v;
            }
    }
    int rc = hpss_split(in, frames, ch, 44100, &h, &p);
    if (rc != 0 || !h || !p) {
        fprintf(stderr, "hpss_split failed ch=%u frames=%zu\n", ch, frames);
        return 1;
    }
    if (!finite_all(h, n) || !finite_all(p, n)) {
        fprintf(stderr, "non-finite output\n");
        return 1;
    }
    if (silent) {
        if (max_err(h, in, n) != 0 || max_err(p, in, n) != 0) {
            fprintf(stderr, "silence did not split to zero\n");
            return 1;
        }
    } else {
        float *sum = malloc(n * sizeof *sum);
        if (!sum)
            return 1;
        for (size_t i = 0; i < n; ++i)
            sum[i] = h[i] + p[i];
        double e = max_err(sum, in, n);
        free(sum);
        if (e > 2e-5) {
            fprintf(stderr, "reconstruction error %.3g ch=%u frames=%zu\n", e, ch,
                    frames);
            return 1;
        }
    }
    free(h);
    free(p);
    free(in);
    return 0;
}

int main(void)
{
    if (run_case(1, 63906, 0)) return 1;
    if (run_case(2, 44100, 0)) return 1;
    if (run_case(1, 1, 0)) return 1;
    if (run_case(1, 4410, 1)) return 1;
    float *h = (float *)1, *p = (float *)1;
    if (hpss_split(NULL, 10, 1, 44100, &h, &p) != -1 || h || p)
        return 1;
    if (hpss_split((const float[]){1}, 1, 3, 44100, &h, &p) != -1)
        return 1;
    puts("hpss dsp ok");
    return 0;
}
'''

with tempfile.TemporaryDirectory() as temp:
    root = Path(temp)
    harness = root / "harness.c"
    harness.write_text(HARNESS)
    exe = root / "hpss-test"
    subprocess.run(
        ["cc", "-O2", "-std=c11", "-I", str(ROOT / "src"), "-o", str(exe),
         str(harness), str(SOURCE), "-lm"],
        check=True,
    )
    subprocess.run([str(exe)], check=True)
