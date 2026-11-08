#ifndef MINIMOD_HOOKS_FFI_H
#define MINIMOD_HOOKS_FFI_H

#include <stdint.h>

/* Hook-site guards, implemented in Rust (rust/crates/minimod/src/hooks.rs). */

int minimod_midi_site_ok(uintptr_t base);

/* The hook engine, from Rust (rust/crates/minimod/src/hooks.rs). */
void *minimod_install_hook(uintptr_t base, uintptr_t site, uintptr_t hook);
int minimod_hooks_touched(void);
void minimod_seal_hooks(void);

/* A decoded MIDI note event, from Rust (rust/crates/minimod/src/midi.rs). */
struct minimod_note {
    uint32_t kind;
    uint32_t note;
    float velocity;
    int32_t external;
    int32_t note_on;
    int32_t note_off;
    int32_t row;
};

int minimod_midi_decode(const unsigned char *event, uint64_t source,
                        struct minimod_note *out);

/* The note hook's state and handler, from Rust
 * (rust/crates/minimod/src/midi.rs). */
struct minimod_midi_state {
    int32_t on;
    int32_t levels;
    uintptr_t pad_ctx;
    uint32_t pad_level_byte;
    int32_t pad_tune_mode;
    int32_t pad_prop_val;
    int32_t pad_prop_pending;
    uint8_t replayed[128];
    uint32_t chromatic_count;
    int32_t chromatic_note;
    int32_t chromatic_delta;
};

typedef void (*minimod_note_event_fn)(void *, void *, void *, uintptr_t);
typedef void (*minimod_pitch_off_fn)(void *, uint64_t, uint64_t, int);

void minimod_midi_handle(uintptr_t app, uintptr_t context, unsigned char *event,
                         uintptr_t source, struct minimod_midi_state *state,
                         minimod_note_event_fn note_event,
                         minimod_pitch_off_fn pitch_off);

#endif
