//! External MIDI chromatic playback while 16 Levels is active.
//!
//! The SDK decodes the app's note event and forwards it; this module decides
//! when to replay it as the selected pad and keeps the counters the device test
//! reads.

use mpc_sample_sdk::{NoteEvent, NoteFlow};

/// The event's type tag for a pad/note event.
pub const PAD_EVENT: u8 = 3;

/// What the MIDI policy carries between events.
///
/// The device test reads the tail fields by offset, so the layout is fixed and
/// `g_midi` stays exported.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MidiState {
    /// Whether the mod is handling notes at all.
    pub on: i32,
    /// Whether 16 Levels is active.
    pub levels: i32,
    /// The context of the pad that last sent an event.
    pub pad_ctx: usize,
    /// The selected pad's note byte.
    pub pad_level_byte: u32,
    /// Whether the pad is in tune mode.
    pub pad_tune_mode: i32,
    /// The property value the pad reported.
    pub pad_prop_val: i32,
    /// Whether that property is waiting to be learned.
    pub pad_prop_pending: i32,
    /// Notes the mod is holding open, so a release can be matched.
    pub replayed: [u8; 128],
    /// How many notes the mod has replayed, for diagnostics and tests.
    pub chromatic_count: u32,
    /// The last note it replayed.
    pub chromatic_note: i32,
    /// That note's row.
    pub chromatic_delta: i32,
}

mpc_sample_sdk::export_static!(#[doc = "The MIDI state, read by the device tests."]
g_midi: mpc_sample_sdk::state::Global<MidiState> = mpc_sample_sdk::state::Global::new(MidiState {
    on: 0,
    levels: 0,
    pad_ctx: 0,
    pad_level_byte: 0,
    pad_tune_mode: 0,
    pad_prop_val: 0,
    pad_prop_pending: 0,
    replayed: [0; 128],
    chromatic_count: 0,
    chromatic_note: 0,
    chromatic_delta: 0,
}));

/// Handle one note event; the SDK forwards whatever this returns.
pub fn note(event: &mut NoteEvent) -> NoteFlow {
    // SAFETY: the MIDI record is owned by the app's input thread; the closure
    // does not take another borrow of it.
    unsafe {
        g_midi.with_mut(|state| {
            if state.on == 0 {
                return NoteFlow::Forward;
            }
            if !event.external {
                state.pad_ctx = event.context;
                if event.kind == PAD_EVENT && event.velocity > 0.0 {
                    state.pad_tune_mode = i32::from(event.mask & 1 == 0);
                    if event.mask & 1 != 0 {
                        state.pad_prop_val = i32::from(event.property);
                        state.pad_level_byte = u32::from(event.note);
                        state.pad_prop_pending = 1;
                    }
                }
            }
            let index = usize::from(event.note);
            let held = state.replayed.get(index).copied().unwrap_or(0) != 0;
            if state.levels != 0
                && event.external
                && event.note <= 127
                && (event.kind == PAD_EVENT || event.note_off)
                && (event.note_on || held)
            {
                // A chromatic note is measured from the sample's own root note.
                let row =
                    i32::from(event.note) - crate::autotrim::pad_root(crate::input::last_pad());
                let pad = u8::try_from(state.pad_level_byte).unwrap_or(0);
                let context = state.pad_ctx;
                if event.replay_as_pad(pad, row, context) {
                    if let Some(slot) = state.replayed.get_mut(index) {
                        *slot = u8::from(event.note_on);
                    }
                    if event.note_on {
                        state.chromatic_count = state.chromatic_count.wrapping_add(1);
                        state.chromatic_note = i32::from(event.note);
                        state.chromatic_delta = row;
                    }
                    return NoteFlow::Replayed;
                }
            }
            NoteFlow::Forward
        })
    }
}

/// Whether the mod handles notes at all.
///
/// Turning the mode off also drops 16 Levels: `ui` calls this with the value
/// `levels_active` returned, so a page that went away cannot leave a mode that
/// reappears when it comes back.
pub fn set_on(on: i32) {
    // SAFETY: the MIDI record is owned by the app's input thread.
    unsafe {
        g_midi.with_mut(|state| {
            let on = i32::from(on != 0);
            state.on = on;
            if on == 0 {
                state.levels = 0;
            }
        });
    }
}

/// Toggle 16 Levels and keep the dispatcher in step.
pub fn toggle_levels() {
    let pad = crate::input::last_pad();
    // SAFETY: the MIDI record is owned by the app's input thread.
    unsafe {
        g_midi.with_mut(|state| {
            state.levels = i32::from(state.levels == 0);
            if state.levels != 0 && (0..16).contains(&pad) {
                state.pad_level_byte = u32::try_from(pad).unwrap_or(0);
            }
            state.on = state.levels;
        });
    }
}

/// Whether 16 Levels is showing and its page predicate agrees.
#[must_use]
pub fn levels_active() -> i32 {
    let screen = crate::paint::screen();
    if screen == 0 {
        return 0;
    }
    // SAFETY: `screen` is the component the paint hook last recorded on this
    // UI thread.
    let visible = unsafe { crate::page::visible(screen) };
    if !crate::page::is_sample_page(screen) || !visible {
        return 0;
    }
    // SAFETY: the MIDI record is owned by the app's input thread.
    unsafe { g_midi.with(|state| state.levels) }
}

#[cfg(test)]
mod tests {
    use super::g_midi;

    #[test]
    fn the_state_starts_without_a_mode() {
        // the tests read a static the process owns.
        // SAFETY: the test is the only accessor while it runs.
        let state = unsafe { g_midi.with(|state| *state) };
        assert_eq!(state.on, 0);
        assert_eq!(state.levels, 0);
        assert_eq!(state.replayed.len(), 128);
    }
}
