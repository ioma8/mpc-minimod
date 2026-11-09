#ifndef MINIMOD_ENTRIES_FFI_H
#define MINIMOD_ENTRIES_FFI_H

#include <stddef.h>
#include <stdint.h>

/* The small hooks, implemented in Rust
 * (rust/crates/minimod/src/entries.rs). */

void minimod_entries_originals(void *ctor, void *proc, void *pitch, void *repaint,
                               void *assign, void *writer);

void *minimod_hook_sixteen_ctor(void *a, void *b, void *c, void *d);
void minimod_hook_sixteen_proc(void *self, int32_t index, void *out);
void minimod_hook_pitch_off(void *a0, uint64_t a1, uint64_t a2, int32_t a3);
void minimod_hook_repaint(void *component, uint64_t a, uint64_t b);
void minimod_hook_str_assign(void *dest, const char *text);
void minimod_hook_hpss_writer(void *closure, void *block);

uintptr_t minimod_sixteen_screen(void);
uintptr_t minimod_filter_dest(void);

#endif
