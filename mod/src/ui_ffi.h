#ifndef MINIMOD_UI_FFI_H
#define MINIMOD_UI_FFI_H

#include <stdint.h>

/* The button hook and the state it keeps, implemented in Rust
 * (rust/crates/minimod/src/ui.rs). */

struct minimod_ui_state {
    int32_t shift;
    int32_t erase;
    int32_t trim_step;
    int32_t b3_down;
    int32_t plus_held;
    int32_t minus_held;
    int32_t octave;
    float mark_v0;
    float mark_v1;
    uintptr_t last_press_obj;
    uint8_t event[16];
    int32_t event_valid;
};

struct minimod_ui_hooks {
    void (*forward)(void *event);
    void (*toggle_levels)(void);
    void (*menu_open)(void);
    void (*menu_close)(void);
    void (*menu_activate)(void);
    void (*filter_text)(const char *text);
    void (*midi_sync)(int32_t levels);
    int32_t (*levels_active)(void);
    int32_t (*page_visible)(void);
    int32_t (*menu_open_state)(void);
    int32_t (*bass_edit_state)(void);
    int32_t (*busy_state)(void);
    int32_t (*spectral_state)(void);
    void (*restore_chop)(void);
    void (*repaint)(void);
    void (*clear_marks)(void);
    int32_t (*bump_octave)(int32_t steps);
    void (*run_autotrim)(void);
    void (*release_held)(void);
    uintptr_t (*read_pointer)(void *event);
};

struct minimod_ui_state *minimod_ui_state(void);
int32_t minimod_hook_buttons(void *event, const struct minimod_ui_hooks *hooks);

#endif
