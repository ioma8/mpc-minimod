#ifndef MINIMOD_FMBASS_SAMPLE_H
#define MINIMOD_FMBASS_SAMPLE_H

#include <stddef.h>
#include <stdint.h>

int fmbass_sample_render(uintptr_t app_base, unsigned rate, float harmonics,
                         float drive, float cutoff, float **output,
                         size_t *frames);

#endif
