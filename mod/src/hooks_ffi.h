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

/* The 16 Levels replay rewrite, from Rust (rust/crates/minimod/src/midi.rs). */
struct minimod_saved {
    uint16_t mask;
    uint8_t property;
    uint8_t note;
};

typedef void (*minimod_pitch_off_fn)(void *, uint64_t, uint64_t, int);

struct minimod_saved minimod_midi_replay(unsigned char *event,
                                         uint8_t pad_level_byte, int32_t row,
                                         minimod_pitch_off_fn pitch_off);
void minimod_midi_restore(unsigned char *event, struct minimod_saved saved);

#endif
