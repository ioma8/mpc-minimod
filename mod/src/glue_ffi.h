#ifndef MINIMOD_GLUE_FFI_H
#define MINIMOD_GLUE_FFI_H

#include <stdint.h>

/* The loader's checks, implemented in Rust (rust/crates/minimod/src/glue.rs). */

int32_t minimod_firmware_ok(uintptr_t base);
uintptr_t minimod_find_main(void);

#endif
