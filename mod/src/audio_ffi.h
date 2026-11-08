#ifndef MINIMOD_AUDIO_FFI_H
#define MINIMOD_AUDIO_FFI_H

#include <stdint.h>

/* Stream geometry, implemented in Rust (rust/crates/minimod/src/audio.rs). */

int minimod_audio_channels(uintptr_t self, int input_side);

#endif
