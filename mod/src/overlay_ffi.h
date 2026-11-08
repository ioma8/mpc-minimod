#ifndef MINIMOD_OVERLAY_FFI_H
#define MINIMOD_OVERLAY_FFI_H

#include <stdint.h>

/* The Sample page overlays, implemented in Rust
 * (rust/crates/minimod/src/overlay.rs). */

struct minimod_overlay_hooks {
    void (*original)(void *component, void *graphics);
    void (*commit_trim)(void);
    void (*spectral_tick)(void);
    void (*finalize)(void);
    void (*restore_chop)(void);
    void (*looper_pump)(void);
    void (*menu_draw)(void *graphics);
};

struct minimod_overlay_flags {
    int32_t pending_cc;
    int32_t restore_pending;
};

struct minimod_overlay_outcome {
    int32_t sample_page;
    int32_t screen_changed;
};

struct minimod_overlay_outcome minimod_overlay_paint(
    uintptr_t component, uintptr_t graphics, uintptr_t base,
    const struct minimod_overlay_hooks *hooks, struct minimod_overlay_flags *flags,
    uintptr_t *screen_pointer, int32_t shift, int32_t chop_mode,
    int32_t looper_open, int32_t menu_open, const float *marks);

#endif
