#ifndef MINIMOD_BUTTONS_FFI_H
#define MINIMOD_BUTTONS_FFI_H

#include <stdint.h>

/* The button ladder, implemented in Rust
 * (rust/crates/minimod/src/buttons.rs). */

#define MINIMOD_KEY_PLUS 0x36u

enum minimod_button_action {
    MINIMOD_ACTION_FORWARD = 0,
    MINIMOD_ACTION_CONSUME = 1,
    MINIMOD_ACTION_SHIFT = 2,
    MINIMOD_ACTION_ERASE = 3,
    MINIMOD_ACTION_LOOPER = 4,
    MINIMOD_ACTION_MENU_OPEN = 5,
    MINIMOD_ACTION_MENU_CLOSE = 6,
    MINIMOD_ACTION_MENU_ACTIVATE = 7,
    MINIMOD_ACTION_AUTOTRIM = 8,
    MINIMOD_ACTION_BUMP_OCTAVE = 9,
    MINIMOD_ACTION_TOGGLE_LEVELS = 10,
    MINIMOD_ACTION_MENU_ENCODER = 11,
    MINIMOD_ACTION_RESET_PITCH = 12
};

int32_t minimod_button_action(uint32_t code, uint32_t down, int32_t shift,
                              int32_t erase, int32_t looper, int32_t menu,
                              int32_t data_down, int32_t busy, int32_t bass_edit,
                              int32_t spectral, int32_t levels,
                              int32_t page_visible, int32_t plus_held,
                              int32_t minus_held, uint32_t *value);

#endif
