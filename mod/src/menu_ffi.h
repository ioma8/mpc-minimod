#ifndef MINIMOD_MENU_FFI_H
#define MINIMOD_MENU_FFI_H

#include <stdint.h>

/* The effects menu, implemented in Rust (rust/crates/minimod/src/menu.rs). */

struct minimod_menu {
    int32_t open;
    int32_t busy;
    int32_t bass_edit;
    int32_t data_down;
    int32_t selection;
    int32_t restore_pending;
};

struct minimod_menu_hooks {
    void (*repaint)(void);
    void (*start_split)(void);
    void (*start_effect)(int32_t row);
    void (*start_spectral)(void);
    void (*start_bass)(void);
    void (*restore_chop)(void);
    void (*start_bass_page)(void);
    int32_t (*start_looper)(void);
    void (*set_status)(const char *text);
};

int32_t minimod_menu_open(struct minimod_menu *state,
                          const struct minimod_menu_hooks *hooks,
                          int32_t page_visible);
int32_t minimod_menu_close(struct minimod_menu *state,
                           const struct minimod_menu_hooks *hooks);
void minimod_menu_activate(struct minimod_menu *state,
                           const struct minimod_menu_hooks *hooks);
void minimod_menu_restore_chop(struct minimod_menu *state,
                               const struct minimod_menu_hooks *hooks,
                               int32_t allowed);

#endif
