#ifndef MINIMOD_CLOCK_FFI_H
#define MINIMOD_CLOCK_FFI_H

#include <stdint.h>

/* Milliseconds since boot, from Rust (rust/crates/minimod/src/clock.rs). */
int64_t minimod_now_ms(void);

#endif
