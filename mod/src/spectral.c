#include "spectral.h"
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define N 1024
#define HOP 512
#define BINS (N / 2 + 1)
#define PI 3.14159265358979323846f
#define MAX_FRAMES 65536

struct complex { float r, i; };

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

static float median_of(float *scratch, const float *src, unsigned n)
{
    memcpy(scratch, src, n * sizeof(float));
    qsort(scratch, n, sizeof(float), cmp_float);
    return scratch[n / 2];
}

static float env_max(const float *mono, size_t s, unsigned hop)
{
    float m = 0;
    for (unsigned j = 0; j < hop; ++j) {
        float x = fabsf(mono[s + j]);
        if (x > m) m = x;
    }
    return m;
}

/* Onset detection.
 *
 * Strength is spectral novelty, not amplitude: the bins are split at 250 Hz and
 * 2 kHz, each band's positive log-magnitude flux is divided by that band's own
 * mean, and the three are summed.  A kick, a snare and a hat contribute
 * equally, and a quiet hit counts like a loud one.
 *
 * A frame is an onset when it is a local maximum, clears
 * delta + 2.5 * median(+/-0.5 s) - delta is a tenth of the mean novelty, which
 * keeps a quiet passage from firing on noise - and stands proud of the valley
 * since the previous onset by 0.8 * mean novelty, which keeps the tail of a
 * loud hit from counting as another hit.
 *
 * The boundary then goes where the 1 ms envelope of the waveform passes 15% of
 * the local transient peak, i.e. on the attack itself.  At most max_positions
 * onsets are used, in time order; anything later stays inside the last slice.
 */
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

    unsigned nf = (unsigned)((frames + HOP - 1) / HOP);
    if (nf > MAX_FRAMES) nf = MAX_FRAMES;

    float *mag = malloc((size_t)nf * BINS * sizeof(float));
    float *strength = calloc(nf, sizeof(float));
    float *window = malloc(N * sizeof(float));
    struct complex *a = malloc(N * sizeof(struct complex));
    if (!mag || !strength || !window || !a) {
        free(mag); free(strength); free(window); free(a); free(mono);
        return 0;
    }
    for (unsigned j = 0; j < N; ++j)
        window[j] = .5f - .5f * cosf(2.f * PI * j / (N - 1));

    for (unsigned f = 0; f < nf; ++f) {
        size_t start = (size_t)f * HOP;
        for (unsigned j = 0; j < N; ++j) {
            size_t t = start + j;
            a[j] = (struct complex){t < frames ? mono[t] * window[j] : 0.f, 0};
        }
        fft(a);
        for (unsigned b = 0; b < BINS; ++b)
            mag[(size_t)f * BINS + b] = hypotf(a[b].r, a[b].i);
    }

    unsigned e1 = (unsigned)(250.0 * N / rate);
    unsigned e2 = (unsigned)(2000.0 * N / rate);
    if (e1 < 1) e1 = 1;
    if (e1 >= BINS) e1 = BINS - 1;
    if (e2 <= e1) e2 = e1 + 1;
    if (e2 >= BINS) e2 = BINS - 1;
    unsigned edges[4] = {0, e1, e2, BINS};

    for (unsigned band = 0; band < 3; ++band) {
        unsigned lo = edges[band], hi = edges[band + 1];
        float *band_flux = calloc(nf, sizeof(float));
        if (!band_flux) {
            free(mag); free(strength); free(window); free(a); free(mono);
            return 0;
        }
        double total = 0;
        for (unsigned f = 0; f < nf; ++f) {
            float sum = 0;
            if (f) {
                for (unsigned b = lo; b < hi; ++b) {
                    float cur = log1pf(mag[(size_t)f * BINS + b]);
                    float prev = log1pf(mag[(size_t)(f - 1) * BINS + b]);
                    float d = cur - prev;
                    if (d > 0) sum += d;
                }
            }
            band_flux[f] = sum;
            total += sum;
        }
        float mean = nf ? (float)(total / nf) : 0.f;
        if (mean <= 1e-9f) mean = 1e-9f;
        for (unsigned f = 0; f < nf; ++f)
            strength[f] += band_flux[f] / mean;
        free(band_flux);
    }

    double ssum = 0;
    for (unsigned f = 0; f < nf; ++f) ssum += strength[f];
    float smean = nf ? (float)(ssum / nf) : 0.f;
    float delta = .10f * smean;
    float prominence = .80f * smean;

    unsigned radius = (unsigned)(.5 * rate / HOP + .5);
    if (radius < 1) radius = 1;
    unsigned gap = (unsigned)ceilf(.040f * rate / HOP);
    if (gap < 1) gap = 1;

    float *scratch = malloc((2 * radius + 2) * sizeof(float));
    unsigned *peaks = malloc(nf * sizeof(unsigned));
    if (!scratch || !peaks) {
        free(scratch); free(peaks); free(mag); free(strength); free(window);
        free(a); free(mono);
        return 0;
    }

    unsigned npk = 0, npk_full = 0;
    long long last = -1000000;
    float valley = 1e30f;
    for (unsigned f = 0; f < nf; ++f) {
        float v = strength[f];
        float left = f ? strength[f - 1] : -1e30f;
        float right = f + 1 < nf ? strength[f + 1] : -1e30f;
        unsigned lo = f > radius ? f - radius : 0;
        unsigned hi = f + radius < nf - 1 ? f + radius : nf - 1;
        float med = median_of(scratch, strength + lo, hi - lo + 1);
        float thresh = delta + 2.5f * med;
        /* the last frames analyse zero padding, which looks like a transient */
        int is_peak = v > left && v > right && v >= thresh && f + 2 < nf;
        if (is_peak && (long long)f - last >= (long long)gap &&
            (npk == 0 || v - valley >= prominence)) {
            if (npk < max_positions)
                peaks[npk++] = f;
            else
                npk_full++;
            last = f;
            valley = 1e30f;
        } else if (v < valley) {
            valley = v;
        }
        /* keep counting past the cap so detected_out stays truthful */
        if (npk == max_positions)
            npk_full = 1;
    }

    unsigned hop = (unsigned)(.001 * rate);
    if (hop < 1) hop = 1;
    unsigned span = (unsigned)(.020 * rate);
    unsigned out = 0;
    for (unsigned i = 0; i < npk && out < max_positions; ++i) {
        size_t pos = (size_t)peaks[i] * HOP;
        size_t a0 = pos > span ? pos - span : 0;
        size_t b0 = pos + span < frames ? pos + span : frames;
        size_t cells = (b0 - a0) / hop;
        if (cells < 4) {
            positions[out++] = (unsigned)pos;
            continue;
        }
        float best = 0;
        unsigned besti = 0;
        for (unsigned k = 0; k < cells; ++k) {
            float m = env_max(mono, a0 + (size_t)k * hop, hop);
            if (m > best) { best = m; besti = k; }
        }
        unsigned onset = besti;
        float level = .15f * best;
        while (onset > 0 &&
               env_max(mono, a0 + (size_t)(onset - 1) * hop, hop) > level)
            onset--;
        unsigned p = (unsigned)(a0 + (size_t)onset * hop);
        if (out && p - positions[out - 1] < (unsigned)(.040 * rate))
            continue;
        positions[out++] = p;
    }

    if (detected_out) *detected_out = npk + npk_full;

    if (out && positions[0] > (unsigned)(.050 * rate) && out < max_positions) {
        for (unsigned i = out; i > 0; --i)
            positions[i] = positions[i - 1];
        positions[0] = 0;
        out++;
    }

    free(peaks); free(scratch); free(mag); free(strength); free(window);
    free(a); free(mono);
    return (int)out;
}
