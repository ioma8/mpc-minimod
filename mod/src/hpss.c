#include "hpss.h"
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define N 2048
#define HOP 512
#define BINS (N / 2 + 1)
#define PI 3.14159265358979323846f
struct complex { float r, i; };

static size_t reflect(long long i, size_t n)
{
    if (n <= 1) return 0;
    long long period = 2 * (long long)(n - 1);
    i %= period;
    if (i < 0) i += period;
    return (size_t)(i < (long long)n ? i : period - i);
}

static void fft(struct complex *a, int inverse)
{
    for (unsigned i = 1, j = 0; i < N; ++i) {
        unsigned bit = N >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) { struct complex t = a[i]; a[i] = a[j]; a[j] = t; }
    }
    for (unsigned len = 2; len <= N; len <<= 1) {
        for (unsigned j = 0; j < len / 2; ++j) {
            float angle = (inverse ? 2.f : -2.f) * PI * j / len;
            float wr = cosf(angle), wi = sinf(angle);
            for (unsigned i = j; i < N; i += len) {
                struct complex u = a[i], v = a[i + len / 2];
                float r = wr * v.r - wi * v.i, im = wr * v.i + wi * v.r;
                a[i] = (struct complex){u.r + r, u.i + im};
                a[i + len / 2] = (struct complex){u.r - r, u.i - im};
            }
        }
    }
    if (inverse) for (unsigned i = 0; i < N; ++i) { a[i].r /= N; a[i].i /= N; }
}

static void insert(float *sorted, unsigned count, float value)
{
    while (count && sorted[count - 1] > value) {
        sorted[count] = sorted[count - 1]; --count;
    }
    sorted[count] = value;
}

/* Sliding sorted window: one removal and insertion per step, reflected edges. */
static void medians(const float *in, size_t length, size_t stride,
                    unsigned width, float *out, size_t out_stride, float *sorted)
{
    long long radius = width / 2;
    for (unsigned j = 0; j < width; ++j)
        insert(sorted, j, in[reflect((long long)j - radius, length) * stride]);
    for (size_t i = 0; i < length; ++i) {
        out[i * out_stride] = sorted[width / 2];
        if (i + 1 == length) break;
        float old = in[reflect((long long)i - radius, length) * stride];
        unsigned j = 0;
        while (j + 1 < width && sorted[j] != old) ++j;
        memmove(sorted + j, sorted + j + 1, (width - j - 1) * sizeof(float));
        insert(sorted, width - 1, in[reflect((long long)i + radius + 1, length) * stride]);
    }
}

static unsigned odd_width(float value)
{
    unsigned n = (unsigned)floorf(value + .5f);
    return n | 1u;
}

int hpss_split(const float *input, size_t frames, unsigned channels,
               unsigned rate, float **harmonic, float **percussive)
{
    if (!harmonic || !percussive || harmonic == percussive) return -1;
    *harmonic = NULL; *percussive = NULL;
    if (!input || !frames || channels < 1 || channels > 2 || rate < 8000 || rate > 192000)
        return -1;
    if (frames > (SIZE_MAX - 2 * N) / (channels * sizeof(float))) return -1;
    size_t nf = 1 + (frames + N) / HOP;
    if (nf > SIZE_MAX / BINS / sizeof(struct complex)) return -1;
    size_t cells = nf * BINS;
    size_t samples = frames * channels;
    for (size_t i = 0; i < samples; ++i) if (!isfinite(input[i])) return -1;
    unsigned kt = odd_width(.476f * rate / HOP);
    unsigned kf = odd_width(366.f * N / rate);
    struct complex *spec = malloc(cells * sizeof(*spec));
    float *mag = malloc(cells * sizeof(float)), *hm = malloc(cells * sizeof(float));
    float *weights = calloc(frames, sizeof(float));
    float *h = calloc(samples, sizeof(float)), *p = calloc(samples, sizeof(float));
    float *sorted = malloc((kt > kf ? kt : kf) * sizeof(float));
    if (!spec || !mag || !hm || !weights || !h || !p || !sorted) goto fail;
    float win[N], pm[BINS];
    struct complex a[N], other[N];
    for (unsigned j = 0; j < N; ++j) win[j] = .5f - .5f * cosf(2.f * PI * j / (N - 1));
    for (size_t f = 0; f < nf; ++f) for (unsigned j = 0; j < N; ++j) {
        long long t = (long long)(f * HOP) + j - N;
        if (t >= 0 && (size_t)t < frames) weights[t] += win[j] * win[j];
    }
    for (unsigned c = 0; c < channels; ++c) {
        for (size_t f = 0; f < nf; ++f) {
            for (unsigned j = 0; j < N; ++j) {
                size_t t = reflect((long long)(f * HOP) + j - N, frames);
                a[j] = (struct complex){input[t * channels + c] * win[j], 0};
            }
            fft(a, 0);
            for (unsigned b = 0; b < BINS; ++b) {
                spec[f * BINS + b] = a[b];
                mag[f * BINS + b] = hypotf(a[b].r, a[b].i);
            }
        }
        for (unsigned b = 0; b < BINS; ++b)
            medians(mag + b, nf, BINS, kt, hm + b, BINS, sorted);
        for (size_t f = 0; f < nf; ++f) {
            medians(mag + f * BINS, BINS, 1, kf, pm, 1, sorted);
            for (unsigned b = 0; b < BINS; ++b) {
                float hv = hm[f * BINS + b], pv = .55f * pm[b];
                float h2 = hv * hv, p2 = pv * pv;
                float hp = h2 * h2 * h2, pp = p2 * p2 * p2;
                float mask = pp / fmaxf(hp + pp, 1e-12f);
                struct complex z = spec[f * BINS + b];
                a[b] = (struct complex){z.r * mask, z.i * mask};
                other[b] = (struct complex){z.r * (1.f - mask), z.i * (1.f - mask)};
            }
            for (unsigned b = BINS; b < N; ++b) {
                a[b] = (struct complex){a[N-b].r, -a[N-b].i};
                other[b] = (struct complex){other[N-b].r, -other[N-b].i};
            }
            fft(a, 1); fft(other, 1);
            for (unsigned j = 0; j < N; ++j) {
                long long t = (long long)(f * HOP) + j - N;
                if (t < 0 || (size_t)t >= frames) continue;
                size_t idx = (size_t)t * channels + c;
                p[idx] += a[j].r * win[j]; h[idx] += other[j].r * win[j];
            }
        }
    }
    for (size_t t = 0; t < frames; ++t) for (unsigned c = 0; c < channels; ++c) {
        h[t * channels + c] /= fmaxf(weights[t], 1e-9f);
        p[t * channels + c] /= fmaxf(weights[t], 1e-9f);
    }
    free(spec); free(mag); free(hm); free(weights); free(sorted);
    *harmonic = h; *percussive = p;
    return 0;
fail:
    free(spec); free(mag); free(hm); free(weights); free(sorted); free(h); free(p);
    return -1;
}
