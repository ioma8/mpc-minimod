#ifndef MINIMOD_LEVELS_FFI_H
#define MINIMOD_LEVELS_FFI_H

#include <stdint.h>

/* The 16 Levels octave shift, implemented in Rust
 * (rust/crates/minimod/src/levels.rs). */

int minimod_levels_shift(int current, int delta, int *out);
int minimod_levels_pitch_offset(int applied, int shift);

typedef void *(*minimod_sixteen_ctor_fn)(void *, void *, void *, void *);
typedef void (*minimod_sixteen_proc_fn)(void *, int, void *);
typedef void (*minimod_pitch_off_fn)(void *, uint64_t, uint64_t, int);

void *minimod_sixteen_ctor(minimod_sixteen_ctor_fn original, uintptr_t a,
                           uintptr_t b, uintptr_t c, uintptr_t d);
void minimod_sixteen_proc(minimod_sixteen_proc_fn original, uintptr_t self,
                          int index, uintptr_t out);
void minimod_pitch_off(minimod_pitch_off_fn original, uintptr_t a, uint64_t b,
                       uint64_t c, int applied, int shift);

#endif
