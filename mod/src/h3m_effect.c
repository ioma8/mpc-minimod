#include "h3m_effect.h"

#include <math.h>
#include <stdint.h>
#include <stdlib.h>

#define H3M_VINYL_FACTORY 0x3c43cf0 /* Ghidra 0x3d43cf0 */
#define H3M_DECIMATOR_FACTORY 0x3c457a0 /* Ghidra 0x3d457a0 */
#define H3M_REVERB_FACTORY 0x3c445b0 /* Ghidra 0x3d445b0 */
#define H3M_BLOCK_FRAMES 512
#define REVERB_TAIL_SECONDS 4

struct audio_context {
    float sample_rate;
    uint32_t reserved;
    int32_t block_size;
};

typedef void *(*effect_factory)(void *, const struct audio_context *);
typedef void (*effect_setup)(void *, long);
typedef void (*effect_process)(void *, const float *const *, float *const *,
                               long, uint64_t *, long);
typedef void (*effect_destroy)(void *);

int h3m_effect_render(uintptr_t app_base, enum h3m_effect kind,
                     h3m_read_fn read_mem, const float *input, size_t frames,
                     unsigned channels, unsigned rate, float **output,
                     size_t *output_frames)
{
    if (!output || !output_frames)
        return -1;
    *output = NULL;
    *output_frames = 0;
    if (!app_base || !input || !frames || channels < 1 || channels > 2 ||
        rate < 8000 || rate > 192000 || frames > SIZE_MAX / channels / sizeof(float))
        return -1;

    if (kind < H3M_REVERB || kind > H3M_DECIMATOR)
        return -1;
    size_t tail = kind == H3M_REVERB ? (size_t)rate * REVERB_TAIL_SECONDS : 0;
    if (frames > SIZE_MAX - tail || frames + tail > SIZE_MAX / channels / sizeof(float))
        return -1;
    size_t total = frames + tail;
    float *rendered = calloc(total * channels, sizeof(float));
    if (!rendered)
        return -1;

    struct audio_context context = {(float)rate, 0, H3M_BLOCK_FRAMES};
    uintptr_t factory_offset = kind == H3M_VINYL ? H3M_VINYL_FACTORY :
                               kind == H3M_DECIMATOR ? H3M_DECIMATOR_FACTORY :
                               H3M_REVERB_FACTORY;
    effect_factory create = (effect_factory)(app_base + factory_offset);
    void *effect = create(NULL, &context);
    if (!effect) {
        free(rendered);
        return -1;
    }

    void **vtable = 0;
    uintptr_t fn_setup = 0, fn_process = 0, fn_destroy = 0, params_p = 0;
    if (read_mem) {
        /* The factory returned an object; check its class data and slots before
         * calling through them.  A failed check leaks the object but cannot
         * corrupt the host. */
        if (!read_mem((uintptr_t)effect, &vtable, sizeof vtable) || !vtable ||
            (uintptr_t)vtable < app_base ||
            (uintptr_t)vtable >= app_base + 0x12000000ULL ||
            !read_mem((uintptr_t)vtable + 9 * 8, &fn_setup, 8) ||
            !read_mem((uintptr_t)vtable + 11 * 8, &fn_process, 8) ||
            !read_mem((uintptr_t)vtable + 1 * 8, &fn_destroy, 8) ||
            !fn_setup || !fn_process || !fn_destroy ||
            fn_setup < app_base || fn_setup >= app_base + 0x12000000ULL ||
            fn_process < app_base || fn_process >= app_base + 0x12000000ULL ||
            fn_destroy < app_base || fn_destroy >= app_base + 0x12000000ULL ||
            !read_mem((uintptr_t)effect + 0x18, &params_p, 8) || !params_p ||
            (params_p & 3)) {
            free(rendered);
            return -1;
        }
        float params_probe[6];
        if (!read_mem(params_p, params_probe, sizeof params_probe)) {
            free(rendered);
            return -1;
        }
    } else {
        vtable = *(void ***)effect;
        fn_setup = (uintptr_t)vtable[9];
        fn_process = (uintptr_t)vtable[11];
        fn_destroy = (uintptr_t)vtable[1];
        params_p = *(uintptr_t *)((char *)effect + 0x18);
    }
    effect_setup setup = (effect_setup)fn_setup;
    effect_process process = (effect_process)fn_process;
    effect_destroy destroy = (effect_destroy)fn_destroy;
    float *params = (float *)params_p;
    if (kind == H3M_VINYL) {
        /* Mild drive and a restrained noise level retain the source while
           making the firmware's vinyl coloration audible. */
        params[0] = 0.18f; /* Drive */
        params[5] = 0.12f; /* Noise Level */
    } else if (kind == H3M_DECIMATOR) {
        /* About 8-bit quantization and a reduced sample rate. */
        params[0] = 0.58f; /* Bit Depth */
        params[4] = 0.70f; /* Sample Rate */
    }
    /* This slot takes the H3 audio context, not a boolean. The base setup
       callback forwards it to the sample-rate/block-size initializer. */
    setup(effect, (long)&context);

    float input_block[2][H3M_BLOCK_FRAMES];
    float output_block[2][H3M_BLOCK_FRAMES];
    const float *input_channels[2] = {input_block[0], input_block[1]};
    float *output_channels[2] = {output_block[0], output_block[1]};
    uint64_t flags = 0;
    for (size_t start = 0; start < total; start += H3M_BLOCK_FRAMES) {
        size_t count = total - start;
        if (count > H3M_BLOCK_FRAMES)
            count = H3M_BLOCK_FRAMES;
        for (size_t i = 0; i < count; ++i) {
            size_t frame = start + i;
            float left = 0.f, right = 0.f;
            if (frame < frames) {
                left = input[frame * channels];
                right = channels == 2 ? input[frame * 2 + 1] : left;
            }
            input_block[0][i] = left;
            input_block[1][i] = right;
        }
        process(effect, input_channels, output_channels, (long)count, &flags, 0);
        for (size_t i = 0; i < count; ++i) {
            size_t frame = start + i;
            if (channels == 2) {
                rendered[frame * 2] = output_block[0][i];
                rendered[frame * 2 + 1] = output_block[1][i];
            } else {
                rendered[frame] = (output_block[0][i] + output_block[1][i]) * 0.5f;
            }
        }
    }
    destroy(effect);

    float peak = 0.f;
    for (size_t i = 0; i < total * channels; ++i) {
        if (!isfinite(rendered[i])) {
            free(rendered);
            return -1;
        }
        float magnitude = fabsf(rendered[i]);
        if (magnitude > peak)
            peak = magnitude;
    }
    if (peak > 0.98f) {
        float scale = 0.98f / peak;
        for (size_t i = 0; i < total * channels; ++i)
            rendered[i] *= scale;
        peak = 0.98f;
    }

    float threshold = fmaxf(1e-5f, peak * 1e-4f);
    size_t last_tail_frame = frames;
    for (size_t frame = frames; frame < total; ++frame) {
        int audible = 0;
        for (unsigned channel = 0; channel < channels; ++channel) {
            if (fabsf(rendered[frame * channels + channel]) > threshold) {
                audible = 1;
                break;
            }
        }
        if (audible)
            last_tail_frame = frame + 1;
    }
    size_t margin = rate / 10;
    if (last_tail_frame < total && margin < total - last_tail_frame)
        last_tail_frame += margin;
    else
        last_tail_frame = total;
    *output = rendered;
    *output_frames = last_tail_frame;
    return 0;
}
