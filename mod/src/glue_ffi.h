#ifndef MINIMOD_GLUE_FFI_H
#define MINIMOD_GLUE_FFI_H

#include <stdint.h>

/* The loader's checks, implemented in Rust (rust/crates/minimod/src/glue.rs). */

int32_t minimod_firmware_ok(uintptr_t base);
uintptr_t minimod_find_main(void);

int32_t minimod_paint_install(uintptr_t base, const void *handler);
void (*minimod_paint_original(void))(void *, void *);

size_t minimod_site_count(void);
uintptr_t minimod_site_offset(size_t index);
const void *minimod_site_handler(size_t index);
void minimod_site_record(size_t index, uintptr_t original);
int32_t minimod_after_sites(const void *originals);

#endif
