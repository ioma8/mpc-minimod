#ifndef MINIMOD_SELECTION_FFI_H
#define MINIMOD_SELECTION_FFI_H

#include <stdint.h>

/* The Sample page's selected sample, implemented in Rust
 * (rust/crates/minimod/src/selection.rs). */

int minimod_sample_now(uintptr_t base, uintptr_t screen, uintptr_t *screen_out,
                       uintptr_t *state, uintptr_t *sample, unsigned *frames,
                       unsigned *channels, unsigned *rate);

#endif
