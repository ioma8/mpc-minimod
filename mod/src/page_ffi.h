#ifndef MINIMOD_PAGE_FFI_H
#define MINIMOD_PAGE_FFI_H

#include <stdint.h>

/* What the Sample page is showing, implemented in Rust
 * (rust/crates/minimod/src/page.rs). */

int minimod_page_visible(uintptr_t base, uintptr_t screen);
int minimod_page_chop_mode(uintptr_t base, uintptr_t screen);

#endif
