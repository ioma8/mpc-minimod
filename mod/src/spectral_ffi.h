#ifndef MINIMOD_SPECTRAL_FFI_H
#define MINIMOD_SPECTRAL_FFI_H

#include <stddef.h>

/* Onset detection, implemented in Rust
 * (rust/crates/minimod/src/analysis_ffi.rs). */
unsigned spectral_onsets(const float *input, size_t frames, unsigned channels,
                         unsigned rate, unsigned *positions,
                         unsigned max_positions, unsigned *detected_out);

#endif
