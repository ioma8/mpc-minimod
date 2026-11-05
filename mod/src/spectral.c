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
#define CHROMA 12
#define TBANDS 48

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

/* Spectral Chop.
 *
 * A slice is a region, not a note: it must be a musical fragment that can be
 * recombined, so boundaries go where the music changes identity (a new chord,
 * a new phrase, a different drum) and always land on a real attack.
 *
 *   candidates  every frame carrying an attack: per-band log flux, threshold
 *               of 0.1 x mean + 2.0 x local median, 0.5 x mean prominence above
 *               the valley since the last one, 40 ms apart;
 *   region      the shortest slice follows the material's onset density,
 *               0.45 / onsets-per-second clamped to 0.18 .. 0.75 s;
 *   keep        a candidate is kept when the slice is long enough AND the sound
 *               has moved on since the last cut: cosine distance of 12-bin
 *               chroma (chord change) or of 48 log bands (different sound);
 *   position    the steepest 1 ms step within +-15 ms, backed off to 15% of the
 *               transient peak, so the cut sits on the attack;
 *   limits      no cuts in the last 120 ms, 16 slices at most, first in time.
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
    if (!mono) return 0;
    for (size_t t = 0; t < frames; ++t) {
        float sum = 0;
        for (unsigned c = 0; c < channels; ++c) sum += input[t * channels + c];
        mono[t] = sum / channels;
    }

    unsigned nf = (unsigned)((frames + HOP - 1) / HOP);
    if (nf > MAX_FRAMES) nf = MAX_FRAMES;

    float *mag = malloc((size_t)nf * BINS * sizeof(float));
    float *flux = calloc(nf, sizeof(float));
    float *window = malloc(N * sizeof(float));
    struct complex *a = malloc(N * sizeof(struct complex));
    float *chr = calloc((size_t)nf * CHROMA, sizeof(float));
    float *tbr = calloc((size_t)nf * TBANDS, sizeof(float));
    if (!mag || !flux || !window || !a || !chr || !tbr) goto fail;

    for (unsigned j = 0; j < N; ++j)
        window[j] = .5f - .5f * cosf(2.f * PI * j / (N - 1));

    /* 48 log-spaced bands from 60 Hz to 12 kHz, as band index edges */
    unsigned tb_edge[TBANDS + 1];
    {
        float lo = 60.f, hi = rate / 2.f < 12000.f ? rate / 2.f : 12000.f;
        float lr = logf(hi / lo) / TBANDS;
        for (unsigned i = 0; i <= TBANDS; ++i) {
            float freq = lo * expf(lr * i);
            unsigned b = (unsigned)(freq * N / rate);
            if (b >= BINS) b = BINS - 1;
            tb_edge[i] = b;
        }
    }
    unsigned pc_of_bin[BINS];
    int chroma_bin[BINS];
    for (unsigned b = 0; b < BINS; ++b) {
        float freq = (float)b * rate / N;
        chroma_bin[b] = (freq >= 55.f && freq <= 2000.f);
        if (chroma_bin[b]) {
            float midi = 69.f + 12.f * log2f(freq / 440.f);
            int pc = (int)lroundf(midi) % 12;
            if (pc < 0) pc += 12;
            pc_of_bin[b] = (unsigned)pc;
        } else {
            pc_of_bin[b] = 0;
        }
    }

    for (unsigned f = 0; f < nf; ++f) {
        size_t start = (size_t)f * HOP;
        for (unsigned j = 0; j < N; ++j) {
            size_t t = start + j;
            a[j] = (struct complex){t < frames ? mono[t] * window[j] : 0.f, 0};
        }
        fft(a);
        float *m = mag + (size_t)f * BINS;
        for (unsigned b = 0; b < BINS; ++b) m[b] = hypotf(a[b].r, a[b].i);
        for (unsigned b = 0; b < BINS; ++b) {
            if (chroma_bin[b]) chr[(size_t)f * CHROMA + pc_of_bin[b]] += m[b];
        }
        for (unsigned i = 0; i < TBANDS; ++i) {
            float sum = 0;
            for (unsigned b = tb_edge[i]; b < tb_edge[i + 1]; ++b) sum += m[b];
            tbr[(size_t)f * TBANDS + i] = sum;
        }
    }

    /* normalise each frame of chroma and timbre, in log domain */
    for (unsigned f = 0; f < nf; ++f) {
        float *c = chr + (size_t)f * CHROMA;
        float *t = tbr + (size_t)f * TBANDS;
        float nc = 0, nt = 0;
        for (unsigned i = 0; i < CHROMA; ++i) { c[i] = log1pf(c[i]); nc += c[i] * c[i]; }
        for (unsigned i = 0; i < TBANDS; ++i) { t[i] = log1pf(t[i]); nt += t[i] * t[i]; }
        nc = sqrtf(nc); nt = sqrtf(nt);
        if (nc > 1e-9f) for (unsigned i = 0; i < CHROMA; ++i) c[i] /= nc;
        if (nt > 1e-9f) for (unsigned i = 0; i < TBANDS; ++i) t[i] /= nt;
    }

    /* attack novelty: per-band log flux, bands split at 250 Hz and 2 kHz */
    unsigned e1 = (unsigned)(250.f * N / rate), e2 = (unsigned)(2000.f * N / rate);
    if (e1 < 1) e1 = 1; if (e1 >= BINS) e1 = BINS - 1;
    if (e2 <= e1) e2 = e1 + 1; if (e2 >= BINS) e2 = BINS - 1;
    unsigned edges[4] = {0, e1, e2, BINS};
    for (unsigned band = 0; band < 3; ++band) {
        unsigned lo = edges[band], hi = edges[band + 1];
        float *bf = calloc(nf, sizeof(float));
        if (!bf) goto fail;
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
            bf[f] = sum;
            total += sum;
        }
        float mean = nf ? (float)(total / nf) : 0.f;
        if (mean <= 1e-9f) mean = 1e-9f;
        for (unsigned f = 0; f < nf; ++f) flux[f] += bf[f] / mean;
        free(bf);
    }

    double fsum = 0;
    for (unsigned f = 0; f < nf; ++f) fsum += flux[f];
    float fmean = nf ? (float)(fsum / nf) : 0.f;
    float thresh_floor = .10f * fmean;
    float prom = .50f * fmean;

    unsigned radius = (unsigned)(.5 * rate / HOP + .5);
    if (radius < 1) radius = 1;
    unsigned gap = (unsigned)ceilf(.040f * rate / HOP);
    if (gap < 1) gap = 1;
    float *scratch = malloc((2 * radius + 2) * sizeof(float));
    unsigned *hits = malloc(nf * sizeof(unsigned));
    if (!scratch || !hits) goto fail;

    unsigned nhits = 0;
    long long last = -1000000;
    float valley = 1e30f;
    for (unsigned f = 0; f < nf; ++f) {
        float v = flux[f];
        float left = f ? flux[f - 1] : -1e30f;
        float right = f + 1 < nf ? flux[f + 1] : -1e30f;
        unsigned lo = f > radius ? f - radius : 0;
        unsigned hi = f + radius < nf - 1 ? f + radius : nf - 1;
        float med = median_of(scratch, flux + lo, hi - lo + 1);
        if (v > left && v > right && v >= thresh_floor + 2.0f * med &&
            f + 2 < nf && (long long)f - last >= (long long)gap &&
            (nhits == 0 || v - valley >= prom)) {
            hits[nhits++] = f;
            last = f;
            valley = 1e30f;
        } else if (v < valley) {
            valley = v;
        }
    }

    /* shortest region follows the material */
    float duration = (float)frames / (float)rate;
    float density = nhits / (duration > .2f ? duration : .2f);
    float region_s = .45f / (density > .4f ? density : .4f);
    if (region_s < .18f) region_s = .18f;
    if (region_s > .75f) region_s = .75f;
    long min_gap = (long)(region_s * rate / HOP);
    unsigned win = (unsigned)(.2 * rate / HOP);
    if (win < 2) win = 2;
    long end_guard = (long)(.120 * rate / HOP);

    float *cmean = malloc(CHROMA * sizeof(float));
    float *tmean = malloc(TBANDS * sizeof(float));
    unsigned *kept = malloc(nf * sizeof(unsigned));
    if (!cmean || !tmean || !kept) goto fail;

    unsigned nkeep = 0;
    for (unsigned i = 0; i < nhits && nkeep < max_positions; ++i) {
        unsigned f = hits[i];
        if ((long)nf - (long)f < end_guard) continue;
        if (nkeep && (long)f - (long)kept[nkeep - 1] < min_gap) continue;
        if (nkeep) {
            unsigned prev = kept[nkeep - 1];
            unsigned lo = prev > win ? prev - win : 0;
            float nc = 0, nt = 0;
            memset(cmean, 0, CHROMA * sizeof(float));
            memset(tmean, 0, TBANDS * sizeof(float));
            for (unsigned k = lo; k < prev; ++k) {
                for (unsigned b = 0; b < CHROMA; ++b) cmean[b] += chr[(size_t)k * CHROMA + b];
                for (unsigned b = 0; b < TBANDS; ++b) tmean[b] += tbr[(size_t)k * TBANDS + b];
            }
            unsigned cnt = prev - lo;
            if (cnt) {
                for (unsigned b = 0; b < CHROMA; ++b) { cmean[b] /= cnt; nc += cmean[b] * cmean[b]; }
                for (unsigned b = 0; b < TBANDS; ++b) { tmean[b] /= cnt; nt += tmean[b] * tmean[b]; }
            }
            float sc = 0, st = 0;
            unsigned hi = f + win < nf ? f + win : nf;
            for (unsigned k = f; k < hi; ++k) {
                for (unsigned b = 0; b < CHROMA; ++b) sc += chr[(size_t)k * CHROMA + b] * cmean[b];
                for (unsigned b = 0; b < TBANDS; ++b) st += tbr[(size_t)k * TBANDS + b] * tmean[b];
            }
            unsigned c2 = hi - f;
            if (c2) { sc /= c2; st /= c2; }
            float nc2 = 0, nt2 = 0;
            memset(cmean, 0, CHROMA * sizeof(float));
            memset(tmean, 0, TBANDS * sizeof(float));
            for (unsigned k = f; k < hi; ++k) {
                for (unsigned b = 0; b < CHROMA; ++b) cmean[b] += chr[(size_t)k * CHROMA + b];
                for (unsigned b = 0; b < TBANDS; ++b) tmean[b] += tbr[(size_t)k * TBANDS + b];
            }
            if (c2) {
                for (unsigned b = 0; b < CHROMA; ++b) { cmean[b] /= c2; nc2 += cmean[b] * cmean[b]; }
                for (unsigned b = 0; b < TBANDS; ++b) { tmean[b] /= c2; nt2 += tmean[b] * tmean[b]; }
            }
            /* silence on one side is a change, not a match */
            float cdist = (nc > 1e-9f && nc2 > 1e-9f)
                ? 1.f - sc / (sqrtf(nc) * sqrtf(nc2)) : 1.f;
            float tdist = (nt > 1e-9f && nt2 > 1e-9f)
                ? 1.f - st / (sqrtf(nt) * sqrtf(nt2)) : 1.f;
            if (cdist < .05f && tdist < .15f) continue;
        }
        kept[nkeep++] = f;
    }

    unsigned hop = (unsigned)(.001f * rate);
    if (hop < 1) hop = 1;
    unsigned span = (unsigned)(.015f * rate);
    unsigned out = 0;
    for (unsigned i = 0; i < nkeep && out < max_positions; ++i) {
        size_t pos = (size_t)kept[i] * HOP;
        size_t a0 = pos > span ? pos - span : 0;
        size_t b0 = pos + span < frames ? pos + span : frames;
        size_t cells = (b0 - a0) / hop;
        if (cells < 4) { positions[out++] = (unsigned)pos; continue; }
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
        if (out && p - positions[out - 1] < (unsigned)(.040f * rate)) continue;
        positions[out++] = p;
    }

    if (detected_out) *detected_out = nkeep;

    if (out && positions[0] > (unsigned)(.050f * rate) && out < max_positions) {
        for (unsigned i = out; i > 0; --i) positions[i] = positions[i - 1];
        positions[0] = 0;
        out++;
    }

    free(cmean); free(tmean); free(kept); free(scratch); free(hits);
    free(mono); free(mag); free(flux); free(window); free(a); free(chr); free(tbr);
    return (int)out;

fail:
    free(mono); free(mag); free(flux); free(window); free(a); free(chr); free(tbr);
    return 0;
}
