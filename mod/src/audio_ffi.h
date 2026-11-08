#ifndef MINIMOD_AUDIO_FFI_H
#define MINIMOD_AUDIO_FFI_H

#include <stdint.h>

/* Stream geometry, implemented in Rust (rust/crates/minimod/src/audio.rs). */

int minimod_audio_channels(uintptr_t self, int input_side);

typedef void (*minimod_audio_writer_fn)(void *self, void *channels, size_t frames,
                                        int offset);
int minimod_audio_writer(uintptr_t self, float **input, size_t frames, int offset,
                         minimod_audio_writer_fn orig, int preview,
                         float *peak_out);

#endif
