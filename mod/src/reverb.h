#ifndef MINIMOD_REVERB_H
#define MINIMOD_REVERB_H

#include <stddef.h>

/* Render a dry-plus-room copy with its room tail appended. */
int reverb_render(const float *input, size_t frames, unsigned channels,
                  unsigned rate, float **output, size_t *output_frames);

#endif
