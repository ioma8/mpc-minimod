#include "reverb.h"

#include <math.h>
#include <stdint.h>
#include <stdlib.h>

#define COMBS 4
#define ALLPASSES 2
#define PI 3.14159265358979323846f

struct delay {
    float *data;
    size_t size, pos;
};

static void delay_init(struct delay *d, unsigned rate, float seconds, unsigned offset)
{
    d->size = (size_t)(rate * seconds + 0.5f) + offset;
    if (d->size < 1) d->size = 1;
    d->data = calloc(d->size, sizeof(float));
}

static float delay_tick(struct delay *d, float input)
{
    float out = d->data[d->pos];
    d->data[d->pos] = input;
    if (++d->pos == d->size) d->pos = 0;
    return out;
}

static void delay_free(struct delay *d)
{
    free(d->data);
}

int reverb_render(const float *input, size_t frames, unsigned channels,
                  unsigned rate, float **output, size_t *output_frames)
{
    if (!output || !output_frames) return -1;
    *output = NULL;
    *output_frames = 0;
    if (!input || !frames || channels < 1 || channels > 2 ||
        rate < 8000 || rate > 192000 || frames > SIZE_MAX / channels / sizeof(float))
        return -1;

    float rt60 = fminf(1.8f, fmaxf(0.25f, (float)frames / rate * 0.75f));
    size_t tail = (size_t)(rt60 * rate + 0.5f);
    if (frames > SIZE_MAX - tail || frames + tail > SIZE_MAX / channels / sizeof(float))
        return -1;
    size_t total = frames + tail;
    float *out = calloc(total * channels, sizeof(float));
    if (!out) return -1;

    static const float comb_ms[COMBS] = {29.7f, 37.1f, 41.1f, 43.7f};
    static const float allpass_ms[ALLPASSES] = {5.0f, 1.7f};
    struct delay comb[2][COMBS] = {{{0}}};
    struct delay allpass[2][ALLPASSES] = {{{0}}};
    float low[2][COMBS] = {{0}};
    float feedback[COMBS];
    float damp = 1.f - expf(-2.f * PI * 6000.f / rate);

    for (unsigned i = 0; i < COMBS; ++i) {
        float seconds = comb_ms[i] * 0.001f;
        feedback[i] = powf(0.001f, seconds / rt60);
        for (unsigned c = 0; c < channels; ++c)
            delay_init(&comb[c][i], rate, seconds, c * (19 + 2 * i));
    }
    for (unsigned i = 0; i < ALLPASSES; ++i)
        for (unsigned c = 0; c < channels; ++c)
            delay_init(&allpass[c][i], rate, allpass_ms[i] * 0.001f, c * (7 + i));

    int ok = 1;
    for (unsigned c = 0; c < channels; ++c) {
        for (unsigned i = 0; i < COMBS; ++i) ok &= comb[c][i].data != NULL;
        for (unsigned i = 0; i < ALLPASSES; ++i) ok &= allpass[c][i].data != NULL;
    }
    if (!ok) goto fail;

    /* A short pre-delay keeps the direct attack clear; the four combs and two
       allpasses build a soft, stereo-decorrelated room decay. */
    for (unsigned c = 0; c < channels; ++c) {
        struct delay predelay = {0};
        delay_init(&predelay, rate, 0.014f, c * 11);
        if (!predelay.data) { delay_free(&predelay); goto fail; }
        for (size_t t = 0; t < total; ++t) {
            float dry = t < frames ? input[t * channels + c] : 0.f;
            float feed = delay_tick(&predelay, dry);
            float wet = 0.f;
            for (unsigned i = 0; i < COMBS; ++i) {
                float delayed = comb[c][i].data[comb[c][i].pos];
                low[c][i] += damp * (delayed - low[c][i]);
                delay_tick(&comb[c][i], feed + low[c][i] * feedback[i]);
                wet += delayed;
            }
            wet *= 0.25f;
            for (unsigned i = 0; i < ALLPASSES; ++i) {
                float delayed = allpass[c][i].data[allpass[c][i].pos];
                float y = delayed - 0.5f * wet;
                delay_tick(&allpass[c][i], wet + 0.5f * y);
                wet = y;
            }
            out[t * channels + c] = dry * 0.88f + wet * 0.30f;
        }
        delay_free(&predelay);
    }

    /* Leave headroom for a near-full-scale source plus the wet signal. */
    float peak = 0.f;
    for (size_t i = 0; i < total * channels; ++i) {
        if (!isfinite(out[i])) goto fail;
        float a = fabsf(out[i]);
        if (a > peak) peak = a;
    }
    if (peak > 0.98f) {
        float scale = 0.98f / peak;
        for (size_t i = 0; i < total * channels; ++i) out[i] *= scale;
    }
    for (unsigned c = 0; c < channels; ++c) {
        for (unsigned i = 0; i < COMBS; ++i) delay_free(&comb[c][i]);
        for (unsigned i = 0; i < ALLPASSES; ++i) delay_free(&allpass[c][i]);
    }
    *output = out;
    *output_frames = total;
    return 0;

fail:
    for (unsigned c = 0; c < channels; ++c) {
        for (unsigned i = 0; i < COMBS; ++i) delay_free(&comb[c][i]);
        for (unsigned i = 0; i < ALLPASSES; ++i) delay_free(&allpass[c][i]);
    }
    free(out);
    return -1;
}
