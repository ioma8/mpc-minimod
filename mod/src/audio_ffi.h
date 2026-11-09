#ifndef MINIMOD_AUDIO_FFI_H
#define MINIMOD_AUDIO_FFI_H

#include <stddef.h>
#include <stdint.h>

/* The audio hooks and their counters, implemented in Rust
 * (rust/crates/minimod/src/audio.rs). */

void minimod_hook_audio_writer(void *self, void *input, size_t frames, int32_t offset);
void *minimod_pump_thread(void *arg);
void minimod_audio_writer_original(void *original);

extern uint32_t g_audio_out_calls;
extern uint32_t g_bass_preview_blocks;
extern uint32_t g_bass_preview_peak_bits;
extern int32_t g_bass_preview_active;
extern uintptr_t g_paint_comp;

#endif
