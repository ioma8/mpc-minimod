#ifndef MINIMOD_HOOKS_FFI_H
#define MINIMOD_HOOKS_FFI_H

#include <stdint.h>

/* Hook-site guards, implemented in Rust (rust/crates/minimod/src/hooks.rs). */

int minimod_midi_site_ok(uintptr_t base);

#endif
