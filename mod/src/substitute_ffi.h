#ifndef MINIMOD_SUBSTITUTE_FFI_H
#define MINIMOD_SUBSTITUTE_FFI_H

#include <stddef.h>
#include <stdint.h>

/* Feeding the app a rendered buffer, implemented in Rust
 * (rust/crates/minimod/src/substitute.rs). */

typedef void (*minimod_writer)(void *closure, void *block);

void minimod_substitute(uintptr_t closure, unsigned char *block,
                        minimod_writer writer);
void minimod_substitute_arm(uintptr_t buffer, size_t frames,
                            size_t source_frames, unsigned channels);
void minimod_substitute_clear(void);
int minimod_substitute_active(void);
void minimod_substitute_progress(size_t *consumed, size_t *total);

#endif
