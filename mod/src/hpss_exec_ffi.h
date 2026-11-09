#ifndef MINIMOD_HPSS_EXEC_FFI_H
#define MINIMOD_HPSS_EXEC_FFI_H

#include <stddef.h>
#include <stdint.h>

/* The slice command's execute step, implemented in Rust
 * (rust/crates/minimod/src/hpss_exec.rs). */

struct minimod_hpss_arm {
    int32_t armed;
    uintptr_t buffer;
    size_t frames;
    size_t source_frames;
    uint32_t channels;
    uint32_t start;
    uint32_t end;
    int32_t rename;
};

struct minimod_hpss_outcome {
    int32_t write_ok;
};

struct minimod_hpss_outcome minimod_hpss_exec_run(uintptr_t command,
                                                  const struct minimod_hpss_arm *arm,
                                                  void (*execute)(void *command));

#endif
