#ifndef MINIMOD_PATHS_FFI_H
#define MINIMOD_PATHS_FFI_H

#include <stddef.h>
#include <stdint.h>

/* Sample-path resolution, implemented in Rust
 * (rust/crates/minimod/src/paths_ffi.rs).  Same names and signatures as the
 * glue functions they replace, so the call sites did not change. */

int path_from_object(uintptr_t sample, unsigned want, char *out, size_t n);
int wav_path_of(uintptr_t obj, unsigned want, char *out, size_t n);
int hpss_path_for(uintptr_t sample, unsigned frames, unsigned ch,
                  unsigned rate, char *path, size_t pathsz);
int find_wav_by_frames(unsigned want, char *out, size_t n);

#endif
