#ifndef MINIMOD_SAMPLE_FFI_H
#define MINIMOD_SAMPLE_FFI_H

#include <stdint.h>

/* Sample and instrument geometry, implemented in Rust
 * (rust/crates/minimod/src/sample.rs). */

unsigned frames_of_sample(uintptr_t sample);
int page_layers(uintptr_t param, uintptr_t *layers, uintptr_t *instr);

#endif
