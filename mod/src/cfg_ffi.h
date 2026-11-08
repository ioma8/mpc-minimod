#ifndef MINIMOD_CFG_FFI_H
#define MINIMOD_CFG_FFI_H

#include <stdint.h>

/* The looper's persisted settings, implemented in Rust
 * (rust/crates/minimod/src/cfg_ffi.rs). */

int minimod_cfg_load(int *trim_frames, int *master);
void minimod_cfg_save(int trim_frames, uint32_t master);

#endif
