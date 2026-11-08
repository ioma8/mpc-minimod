#ifndef MINIMOD_H3M_EFFECT_H
#define MINIMOD_H3M_EFFECT_H

#include <stddef.h>
#include <stdint.h>

enum h3m_effect {
    H3M_REVERB,
    H3M_VINYL,
    H3M_DECIMATOR
};

/* Render with a native H3 effect; Reverb also appends its natural tail.
 * Reads target-process memory for validation.  When supplied, the effect
 * object, its vtable slots and the parameter block are checked before use; a
 * mismatch fails closed instead of writing through a drifted layout. */
typedef int (*h3m_read_fn)(uintptr_t addr, void *dst, size_t n);

int h3m_effect_render(uintptr_t app_base, enum h3m_effect effect,
                      h3m_read_fn read_mem, const float *input, size_t frames,
                      unsigned channels, unsigned rate, float **output,
                      size_t *output_frames);

#endif
