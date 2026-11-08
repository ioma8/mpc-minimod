#ifndef MINIMOD_PROPS_FFI_H
#define MINIMOD_PROPS_FFI_H

#include <stdint.h>

/* The page's property scan, implemented in Rust
 * (rust/crates/minimod/src/props.rs). */

uintptr_t minimod_prop_of_param(uintptr_t parameter, uintptr_t base);
int32_t minimod_mapped(uintptr_t address);

#endif
