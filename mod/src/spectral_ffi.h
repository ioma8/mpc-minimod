#ifndef MINIMOD_SPECTRAL_FFI_H
#define MINIMOD_SPECTRAL_FFI_H

#include <stdint.h>

/* The spectral chop's frame state, implemented in Rust
 * (rust/crates/minimod/src/spectral.rs).  Keep this layout in step with the
 * Rust struct: a mismatch silently misreads every field after the difference. */

struct minimod_spectral_state {
    int32_t active;
    int32_t phase;
    int32_t slices;
    int64_t deadline;
    uint8_t event[16];
    uintptr_t screen;
    uintptr_t page_state;
    uintptr_t sample;
    uint32_t frames;
    uint32_t channels;
    uint32_t rate;
};

struct minimod_spectral_hooks {
    void (*press_chop)(void);
    void (*set_status)(const char *text);
    void (*repaint)(void);
};

int32_t minimod_spectral_start(struct minimod_spectral_state *state,
                               const uint8_t *event, int32_t length,
                               int32_t busy);
void minimod_spectral_tick(struct minimod_spectral_state *state,
                           void *menu_state, uintptr_t base, uintptr_t screen,
                           const struct minimod_spectral_hooks *hooks);

#endif
