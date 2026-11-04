#ifndef MINIMOD_HPSS_H
#define MINIMOD_HPSS_H
#include <stddef.h>
/* Offline, interleaved float32. Caller frees both outputs. Returns 0 on success.
 * No input mutation, gain normalization or gate. */
int hpss_split(const float *input, size_t frames, unsigned channels,
               unsigned rate, float **harmonic, float **percussive);
#endif
