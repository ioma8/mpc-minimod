#ifndef MINIMOD_PADS_FFI_H
#define MINIMOD_PADS_FFI_H

#include <stdint.h>

/* Pad occupancy, implemented in Rust (rust/crates/minimod/src/pads.rs). */

int minimod_pads_free_pad(uintptr_t base, uintptr_t program);
int minimod_pads_free_estimate(uintptr_t base, uintptr_t program);

#endif
