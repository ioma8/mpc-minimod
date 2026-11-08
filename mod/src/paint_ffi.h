#ifndef MINIMOD_PAINT_FFI_H
#define MINIMOD_PAINT_FFI_H

#include <stdint.h>

/* Drawing and the toast, implemented in Rust
 * (rust/crates/minimod/src/paint.rs). */

void minimod_paint_colour(uintptr_t graphics, uint32_t value);
void minimod_paint_rect(uintptr_t graphics, int32_t x, int32_t y, int32_t width,
                        int32_t height);
void minimod_paint_text(uintptr_t graphics, const char *text, int32_t x,
                        int32_t y, int32_t width, int32_t height, float size);

typedef void (*minimod_repaint_fn)(void *component, uint64_t a, uint64_t b);
void minimod_toast(const unsigned char *text, minimod_repaint_fn repaint,
                   uintptr_t component);
const unsigned char *minimod_toast_text(void);
int minimod_toast_visible(int64_t now);

#endif
