#ifndef MINIMOD_FMBASS_SAMPLE_H
#define MINIMOD_FMBASS_SAMPLE_H

#include <stddef.h>
#include <stdint.h>

/* Investigation probe only — never part of the release build. The
 * implementation is compiled out unless -DMINIMOD_FMBASS_PROBE=1; see
 * docs/re/BASS-SAMPLE.md for why the native lifecycle is unresolved. */
int fmbass_sample_render(uintptr_t app_base, unsigned rate, float harmonics,
                         float drive, float cutoff, float **output,
                         size_t *frames);

#endif
