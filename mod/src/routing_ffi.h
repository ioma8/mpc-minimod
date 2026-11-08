#ifndef MINIMOD_ROUTING_FFI_H
#define MINIMOD_ROUTING_FFI_H

#include <stdint.h>

/* What a control-change event means, implemented in Rust
 * (rust/crates/minimod/src/routing.rs). */

struct minimod_route {
    int32_t kind; /* 0 pass, 1 master, 2 trim, 3 bass, 4 consume */
    int32_t value;
    float amount;
};

struct minimod_route minimod_cc_route(int32_t replaying, uint32_t control,
                                      uint32_t value, float range,
                                      int32_t looper_open, int32_t menu_open,
                                      int32_t bass_edit, int32_t busy,
                                      int32_t spectral_active,
                                      int32_t trim_frames);

#endif
