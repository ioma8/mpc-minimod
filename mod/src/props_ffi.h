#ifndef MINIMOD_PROPS_FFI_H
#define MINIMOD_PROPS_FFI_H

#include <stdint.h>

/* The page's property scan, implemented in Rust
 * (rust/crates/minimod/src/props.rs). */

uintptr_t minimod_prop_of_param(uintptr_t parameter, uintptr_t base);
int32_t minimod_mapped(uintptr_t address);
uintptr_t minimod_find_knob_props(uintptr_t base);
int32_t minimod_prop_references(uintptr_t property, uintptr_t parameter);
uintptr_t minimod_ctl_obj_for(uintptr_t base);
uintptr_t minimod_slider_for_value(uintptr_t screen, uintptr_t base, float percent);
int32_t minimod_prop_listeners_sane(uintptr_t property, uintptr_t base);

#endif
