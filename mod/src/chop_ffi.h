#ifndef MINIMOD_CHOP_FFI_H
#define MINIMOD_CHOP_FFI_H

#include <stdint.h>

/* The app's slice list, implemented in Rust (rust/crates/minimod/src/chop.rs). */

unsigned minimod_slice_count(uintptr_t sample, uintptr_t *begin,
                             uintptr_t *continuation);
int minimod_spectral_verify(uintptr_t sample, const unsigned *positions,
                            unsigned count, unsigned frames);
int minimod_spectral_write_markers(uintptr_t base, uintptr_t sample,
                                   const unsigned *positions, unsigned count,
                                   unsigned frames, uintptr_t screen);
int minimod_spectral_manual(uintptr_t base, uintptr_t screen);

#endif
