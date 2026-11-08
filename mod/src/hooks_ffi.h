#ifndef MINIMOD_HOOKS_FFI_H
#define MINIMOD_HOOKS_FFI_H

#include <stdint.h>

/* Hook-site guards, implemented in Rust (rust/crates/minimod/src/hooks.rs). */

int minimod_midi_site_ok(uintptr_t base);

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

#endif
