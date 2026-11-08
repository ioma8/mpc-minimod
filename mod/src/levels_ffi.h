#ifndef MINIMOD_LEVELS_FFI_H
#define MINIMOD_LEVELS_FFI_H

#include <stdint.h>

/* The 16 Levels octave shift, implemented in Rust
 * (rust/crates/minimod/src/levels.rs). */

int minimod_levels_shift(int current, int delta, int *out);
int minimod_levels_pitch_offset(int applied, int shift);

#endif
