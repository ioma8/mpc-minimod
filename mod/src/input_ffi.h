#ifndef MINIMOD_INPUT_FFI_H
#define MINIMOD_INPUT_FFI_H

#include <stdint.h>

/* The app's input hooks, implemented in Rust
 * (rust/crates/minimod/src/input.rs). */

#define MINIMOD_INPUT_PADS 128

struct minimod_pad_record {
    uintptr_t input;
    int32_t src;
    int32_t pad;
    uintptr_t props;
    float velocity;
    int32_t valid;
};

struct minimod_input {
    uint8_t held_on[MINIMOD_INPUT_PADS];
    uintptr_t held_input[MINIMOD_INPUT_PADS];
    int32_t held_src[MINIMOD_INPUT_PADS];
    struct minimod_pad_record pad_rec[MINIMOD_INPUT_PADS];
    int32_t last_pad;
    uintptr_t props;
    int32_t replaying;
    uintptr_t cs_self;
    uintptr_t data_target;
    int32_t pad_prop_base;
};

struct minimod_input_context {
    int32_t looper_open;
    int32_t spectral_active;
    int32_t preview_active;
    int32_t menu_open;
    int32_t bass_edit;
    int32_t busy;
    int32_t shift;
    int32_t erase;
    void (*repaint)(void);
};

typedef void (*minimod_note_on_fn)(void *, int, int, void *, float);
typedef void (*minimod_note_off_fn)(void *, int, int);
typedef void (*minimod_surface_fn)(void *, int, void *);
typedef void (*minimod_data_signal_fn)(void *, int);

void minimod_input_note_on(struct minimod_input *state, uintptr_t input,
                           int32_t src, int32_t pad, uintptr_t props,
                           float velocity, struct minimod_input_context ctx,
                           minimod_note_on_fn callback);
void minimod_input_note_off(struct minimod_input *state, uintptr_t input,
                            int32_t src, int32_t pad,
                            struct minimod_input_context ctx,
                            minimod_note_off_fn callback);
void minimod_input_release_held(struct minimod_input *state,
                                minimod_note_off_fn callback);
void minimod_input_surface(struct minimod_input *state, uintptr_t self_address,
                           int32_t code, uintptr_t payload,
                           minimod_surface_fn callback);
void minimod_input_data(struct minimod_input *state, int32_t *selection,
                        uintptr_t self_address, int32_t delta,
                        struct minimod_input_context ctx,
                        minimod_data_signal_fn callback);

#endif
