#ifndef MINIMOD_HPSS_FFI_H
#define MINIMOD_HPSS_FFI_H

#include <stddef.h>

/* The HPSS split, implemented in Rust (rust/crates/minimod/src/analysis_ffi.rs). */
int hpss_split(const float *input, size_t frames, unsigned channels,
               unsigned rate, float **harmonic, float **percussive);

#endif
