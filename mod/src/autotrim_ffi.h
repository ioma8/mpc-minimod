#ifndef MINIMOD_AUTOTRIM_FFI_H
#define MINIMOD_AUTOTRIM_FFI_H

#include <stdint.h>

/* The AutoTrim soft key, implemented in Rust
 * (rust/crates/minimod/src/autotrim.rs). */

struct minimod_autotrim_hooks {
    void (*set_control)(void *control, void *surface, int32_t pad, int32_t op,
                        float value);
    void (*repaint)(void *component, uint64_t a, uint64_t b);
};

struct minimod_autotrim_context {
    uintptr_t surface;
    uintptr_t control;
    uintptr_t component;
    int32_t step;
    int32_t last_pad;
};

void minimod_autotrim_run(const struct minimod_autotrim_context *context,
                          const struct minimod_autotrim_hooks *hooks);

#endif
