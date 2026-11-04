#include "spectral.h"
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define N 1024
#define HOP 512
#define BINS (N / 2 + 1)
#define PI 3.14159265358979323846f

struct complex { float r, i; };

struct peak {
    unsigned pos;
    float strength;
};

static void fft(struct complex *a)
{
    for (unsigned i = 1, j = 0; i < N; ++i) {
        unsigned bit = N >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) { struct complex t = a[i]; a[i] = a[j]; a[j] = t; }
    }
    for (unsigned len = 2; len <= N; len <<= 1) {
        for (unsigned j = 0; j < len / 2; ++j) {
            float angle = -2.f * PI * j / len;
            float wr = cosf(angle), wi = sinf(angle);
            for (unsigned i = j; i < N; i += len) {
                struct complex u = a[i], v = a[i + len / 2];
                float r = wr * v.r - wi * v.i, im = wr * v.i + wi * v.r;
                a[i] = (struct complex){u.r + r, u.i + im};
                a[i + len / 2] = (struct complex){u.r - r, u.i - im};
            }
        }
    }
}

static int cmp_float(const void *a, const void *b)
{
    float x = *(const float *)a, y = *(const float *)b;
    return (x > y) - (x < y);
}

int spectral_onsets(const float *input, size_t frames, unsigned channels,
                    unsigned rate, unsigned *positions, unsigned max_positions,
                    unsigned *detected_out)
{
    if (detected_out) *detected_out = 0;
    if (!input || !positions || !max_positions || !frames || !channels ||
        !rate || frames < N / 2)
        return 0;

    float *mono = malloc(frames * sizeof(float));
    if (!mono)
        return 0;
    for (size_t t = 0; t < frames; ++t) {
        float sum = 0;
        for (unsigned c = 0; c < channels; ++c)
            sum += input[t * channels + c];
        mono[t] = sum / channels;
    }

    size_t nf = (frames + HOP - 1) / HOP;
    float *flux = calloc(nf, sizeof(float));
    float *prev = calloc(BINS, sizeof(float));
    float *cur = malloc(BINS * sizeof(float));
    float win[N];
    struct complex a[N];
    if (!flux || !prev || !cur) {
        free(mono); free(flux); free(prev); free(cur);
        return 0;
    }
    for (unsigned j = 0; j < N; ++j)
        win[j] = .5f - .5f * cosf(2.f * PI * j / (N - 1));

    for (size_t f = 0; f < nf; ++f) {
        size_t start = f * HOP;
        for (unsigned j = 0; j < N; ++j) {
            size_t t = start + j;
            a[j] = (struct complex){t < frames ? mono[t] * win[j] : 0.f, 0};
        }
        fft(a);
        for (unsigned b = 0; b < BINS; ++b)
            cur[b] = hypotf(a[b].r, a[b].i);
        if (f) {
            float sum = 0;
            for (unsigned b = 0; b < BINS; ++b) {
                float d = cur[b] - prev[b];
                if (d > 0)
                    sum += d;
            }
            flux[f] = sum;
        }
        memcpy(prev, cur, BINS * sizeof(float));
    }

    float maxf = 0;
    for (size_t f = 0; f < nf; ++f)
        if (flux[f] > maxf)
            maxf = flux[f];
    if (maxf > 0)
        for (size_t f = 0; f < nf; ++f)
            flux[f] /= maxf;

    unsigned radius = (unsigned)(.5 * rate / HOP + .5);
    if (radius < 1) radius = 1;
    unsigned med_len = 2 * radius + 1;
    float *window = malloc(med_len * sizeof(float));
    if (!window) {
        free(mono); free(flux); free(prev); free(cur);
        return 0;
    }

    struct peak *cand = malloc(nf * sizeof(*cand));
    if (!cand) {
        free(window); free(mono); free(flux); free(prev); free(cur);
        return 0;
    }
    unsigned ncand = 0;
    unsigned last_pos = 0;
    int have_last = 0;
    unsigned gap = (unsigned)ceilf(.060f * rate / HOP);
    if (gap < 1) gap = 1;

    for (size_t f = 1; f + 1 < nf; ++f) {
        unsigned n = 0;
        size_t lo = f > radius ? f - radius : 0;
        size_t hi = f + radius < nf - 1 ? f + radius : nf - 1;
        for (size_t k = lo; k <= hi; ++k)
            window[n++] = flux[k];
        qsort(window, n, sizeof(float), cmp_float);
        float median = window[n / 2];
        float threshold = median * 1.8f;
        if (threshold < .30f)
            threshold = 0.35f;
        if (flux[f] < threshold || flux[f] < flux[f - 1] || flux[f] <= flux[f + 1])
            continue;
        unsigned pos = (unsigned)(f * HOP + N / 2);
        if (pos + N / 2 >= frames)
            continue; /* end-of-buffer padding makes a fake transient */
        if (have_last && pos - last_pos < gap * HOP)
            continue;
        cand[ncand].pos = pos;
        cand[ncand].strength = flux[f];
        ncand++;
        last_pos = pos;
        have_last = 1;
    }

    if (detected_out)
        *detected_out = ncand;
    if (!ncand) {
        free(cand); free(window); free(mono); free(flux); free(prev); free(cur);
        return 0;
    }

    unsigned take = ncand < max_positions ? ncand : max_positions;
    for (unsigned i = 0; i < take; ++i) {
        unsigned best = i;
        for (unsigned j = i + 1; j < ncand; ++j)
            if (cand[j].strength > cand[best].strength)
                best = j;
        struct peak t = cand[i];
        cand[i] = cand[best];
        cand[best] = t;
    }

    int need_zero = cand[0].pos > (unsigned)(.050f * rate + .5);
    if (need_zero) {
        if (take == max_positions)
            take--;
        cand[take].pos = 0;
        cand[take].strength = 0;
        take++;
    }

    for (unsigned i = 0; i < take; ++i)
        for (unsigned j = i + 1; j < take; ++j)
            if (cand[j].pos < cand[i].pos) {
                struct peak t = cand[i];
                cand[i] = cand[j];
                cand[j] = t;
            }
    for (unsigned i = 0; i < take; ++i)
        positions[i] = cand[i].pos;

    free(cand); free(window); free(mono); free(flux); free(prev); free(cur);
    return (int)take;
}
