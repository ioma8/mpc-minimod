#ifndef MINIMOD_BASS_SAMPLE_H
#define MINIMOD_BASS_SAMPLE_H

#include <stddef.h>

/* Generate a centered stereo plucked C2 sample. Controls are normalized 0..1. */
#ifdef __cplusplus
extern "C" {
#endif
int bass_sample_render(unsigned rate, float harmonics, float drive,
                       float cutoff, float **output, size_t *frames);

/* Realtime preview API. note() is safe from the pad-input thread; mix() runs
   on the firmware audio writer thread and does not allocate or lock. */
void bass_preview_enable(int enabled);
void bass_preview_set_params(float brightness, float excite, float cutoff);
void bass_preview_note(unsigned pad, float velocity);
float bass_preview_mix(float **channels, size_t frames, unsigned channel_count,
                       unsigned rate);
#ifdef __cplusplus
}
#endif

#endif
