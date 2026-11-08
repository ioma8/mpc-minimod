#ifndef MINIMOD_SPECTRAL_MACHINE_FFI_H
#define MINIMOD_SPECTRAL_MACHINE_FFI_H

#include <stdint.h>

/* The Spectral Chop state machine, implemented in Rust
 * (rust/crates/minimod/src/spectral_machine.rs). */

int minimod_spectral_begin(uintptr_t base, int64_t now, uintptr_t screen,
                           uintptr_t state, uintptr_t sample, uint32_t frames,
                           uint32_t channels, uint32_t rate, const float *samples,
                           void (*press_chop)(void),
                           const unsigned char **error);
int minimod_spectral_step(uintptr_t base, int64_t now, uint32_t *slices,
                          const unsigned char **error);
void minimod_spectral_reset(void);

#endif
