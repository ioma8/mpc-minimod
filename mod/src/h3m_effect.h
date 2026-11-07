#ifndef MINIMOD_H3M_EFFECT_H
#define MINIMOD_H3M_EFFECT_H

#include <stddef.h>
#include <stdint.h>

enum h3m_effect {
    H3M_REVERB,
    H3M_VINYL,
    H3M_DECIMATOR
};

/* Render with a native H3 effect; Reverb also appends its natural tail. */
int h3m_effect_render(uintptr_t app_base, enum h3m_effect effect,
                      const float *input, size_t frames, unsigned channels,
                      unsigned rate, float **output, size_t *output_frames);

#endif
