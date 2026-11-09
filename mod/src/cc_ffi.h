#ifndef MINIMOD_CC_FFI_H
#define MINIMOD_CC_FFI_H

#include <stdint.h>

/* Control-change routing, implemented in Rust
 * (rust/crates/minimod/src/cc.rs). */

struct minimod_cc_hooks {
    void (*original)(void *event, uint32_t raw, float low, float range);
    void (*repaint)(void);
    void (*save)(void);
    void (*notice)(const char *text);
};

struct minimod_cc_context {
    int32_t replaying;
    uint32_t value;
    float range;
    int32_t looper_open;
    int32_t menu_open;
    int32_t bass_edit;
    int32_t busy;
    int32_t spectral_active;
    int32_t trim_frames;
    float knobs[3];
};

struct minimod_cc_outcome {
    int32_t consumed;
    int32_t trim_frames;
    int32_t trim_changed;
    float knobs[3];
    int32_t knobs_changed;
};

struct minimod_cc_outcome minimod_cc_handle(const struct minimod_cc_context *context,
                                            uint32_t control,
                                            const struct minimod_cc_hooks *hooks);

#endif
