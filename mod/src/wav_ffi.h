#ifndef MINIMOD_WAV_FFI_H
#define MINIMOD_WAV_FFI_H

#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

/* WAV parsing and the source-file backup, implemented in Rust
 * (rust/crates/minimod/src/wav_ffi.rs).  The signatures are the ones the glue
 * used before the port, so the call sites did not change. */

int wav_info(const char *path, unsigned *frames, unsigned *rate, unsigned *ch,
             unsigned *dataoff, unsigned *bps, unsigned *format);

int wav_read_f32(const char *path, unsigned want_frames, unsigned want_ch,
                 unsigned want_rate, float **out, unsigned *got_ch,
                 unsigned *got_rate);

int bass_source_backup(const char *path, off_t *data_offset,
                       unsigned char **data, size_t *data_size);

int bass_source_restore(const char *path, off_t data_offset,
                        const unsigned char *data, size_t data_size);

#endif
