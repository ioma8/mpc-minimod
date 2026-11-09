//! The input policy: which screen owns a pad, a surface event or an encoder turn.
//!
//! The SDK decodes the app's events and forwards whatever this module does not
//! consume; nothing here touches a displaced function.

use mpc_sample_sdk::state::Global;
use mpc_sample_sdk::{
    EncoderEvent, EncoderFlow, PadAction, PadEvent, PadFlow, SurfaceEvent, SurfaceFlow,
};

/// Pads the app can address.
pub const PADS: usize = 128;
/// Pads the looper owns; a test keeps it equal to the engine's table.
pub const LOOPER_PADS: i32 = 16;
/// Pads the bass preview listens to; a test keeps it equal to the synth's.
pub const PREVIEW_PADS: i32 = 16;
/// Entries on the effects menu; a test keeps it equal to the menu's rows.
pub const MENU_ITEMS: i32 = 8;

/// The state the input hooks keep.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct State {
    /// Pads whose note-on the app may have seen.
    pub held_on: [u8; PADS],
    /// The input each held pad came from.
    pub held_input: [usize; PADS],
    /// The source each held pad came from.
    pub held_src: [i32; PADS],
    /// The pad pressed last.
    pub last_pad: i32,
    /// The encoder's surface, as the button handler recorded it.
    pub data_target: usize,
}

impl State {
    /// A fresh state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            held_on: [0; PADS],
            held_input: [0; PADS],
            held_src: [0; PADS],
            last_pad: -1,
            data_target: 0,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// The screens and keys the policy reads.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    /// Whether the looper view is open.
    pub looper_open: i32,
    /// Whether a spectral chop is running.
    pub spectral_active: i32,
    /// Whether the effects menu is open.
    pub menu_open: i32,
    /// Whether the bass control page is open.
    pub bass_edit: i32,
    /// Whether a job is running.
    pub busy: i32,
    /// Whether SHIFT is held.
    pub shift: i32,
    /// Whether ERASE is held.
    pub erase: i32,
}

/// The pad index, when it is one the app can address.
fn pad_index(pad: i32) -> Option<usize> {
    usize::try_from(pad).ok().filter(|index| *index < PADS)
}

/// A pad press or release.
pub fn pad(event: PadEvent) -> PadFlow {
    // only the app's input thread touches this state.
    let state = STATE.get_mut();
    if let Some(index) = pad_index(event.pad) {
        match event.action {
            PadAction::Press => {
                state.held_on[index] = 1;
                state.held_input[index] = event.input;
                state.held_src[index] = event.source;
            }
            PadAction::Release => state.held_on[index] = 0,
        }
    }
    let context = context();
    if context.looper_open != 0 && event.pad >= 0 {
        if context.looper_open != 0 && event.action == PadAction::Press && event.pad < LOOPER_PADS {
            let _ = crate::backend::looper::looper_pad(event.pad, context.shift, context.erase);
        }
        return PadFlow::Consume;
    }
    if context.spectral_active != 0 {
        return PadFlow::Consume;
    }
    // While the bass page is open the pads belong to the preview, which the
    // control-surface hook feeds; anything that still arrives here is swallowed
    // so the app cannot start the project's sample under it.
    if context.menu_open != 0 && context.bass_edit != 0 && (0..PREVIEW_PADS).contains(&event.pad) {
        return PadFlow::Consume;
    }
    if event.action == PadAction::Press {
        state.last_pad = event.pad;
    }
    PadFlow::Forward
}

/// A control-surface event: the bass page consumes the pad notes it sees.
#[must_use]
pub fn surface(event: SurfaceEvent) -> SurfaceFlow {
    if bass_pad_event(event) {
        SurfaceFlow::Consume
    } else {
        SurfaceFlow::Forward
    }
}

/// The encoder: drive the open menu, or forward the turn.
#[must_use]
pub fn encoder(event: EncoderEvent) -> EncoderFlow {
    // only the app's input thread touches this state.
    let state = STATE.get();
    let context = context();
    if event.surface != state.data_target
        || (context.menu_open == 0 && context.spectral_active == 0)
    {
        return EncoderFlow::Forward;
    }
    if context.menu_open != 0 && context.busy == 0 && event.delta != 0 && context.bass_edit == 0 {
        let step = if event.delta > 0 { 1 } else { 7 };
        let selection = &mut crate::menu::state_mut().selection;
        *selection = (*selection + step) % MENU_ITEMS;
        // the app's own repaint; the UI thread is here.
        crate::ui::minimod_repaint_screen();
    }
    EncoderFlow::Consume
}

/// The pad the user pressed last, or -1.
#[must_use]
pub fn last_pad() -> i32 {
    // only the UI thread writes it.
    STATE.get().last_pad
}

/// Record the encoder's surface.
pub fn set_data_target(target: usize) {
    // only the UI thread writes the input state.
    STATE.get_mut().data_target = target;
}

/// The state itself.
static STATE: Global<State> = Global::new(State::new());

/// What the policy needs to know about the mod's screens and keys, right now.
fn context() -> Context {
    let menu = crate::menu::state();
    // a plain word, written by the UI thread.
    let spectral = crate::spectral::G_SPECTRAL.get().active;
    Context {
        looper_open: crate::backend::looper::looper_is_open(),
        spectral_active: spectral,
        menu_open: menu.open,
        bass_edit: menu.bass_edit,
        busy: menu.busy,
        shift: i32::from(mpc_sample_sdk::shift()),
        erase: i32::from(mpc_sample_sdk::erase()),
    }
}

/// Feed the bass preview a pad note while its page is open.
fn bass_pad_event(event: SurfaceEvent) -> bool {
    let menu = crate::menu::state();
    if menu.open == 0 || menu.bass_edit == 0 || !(0..PREVIEW_PADS).contains(&event.code) {
        return false;
    }
    if !matches!(event.kind, 0x80 | 0x90 | 0xa0) {
        return false;
    }
    if event.kind == 0x90 && event.velocity > 0 {
        crate::backend::bass::preview_note(
            u32::try_from(event.code).unwrap_or(0),
            f32::from(event.velocity) / 127.0,
        );
    }
    true
}

/// Release every pad the app may think is held, through the app's own path.
pub fn release_held() {
    let state = STATE.get_mut();
    for pad in 0..PADS {
        if state.held_on[pad] == 0 {
            continue;
        }
        state.held_on[pad] = 0;
        mpc_sample_sdk::sites::send_note_off(
            state.held_input[pad],
            state.held_src[pad],
            i32::try_from(pad).unwrap_or(0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{LOOPER_PADS, MENU_ITEMS, PREVIEW_PADS, State};

    #[test]
    fn a_fresh_state_remembers_nothing() {
        let state = State::new();
        assert_eq!(state.last_pad, -1);
        assert!(state.held_on.iter().all(|&held| held == 0));
        assert_eq!(MENU_ITEMS, 8);
    }

    #[test]
    fn the_shared_counts_match_their_owners() {
        assert_eq!(
            usize::try_from(LOOPER_PADS).unwrap_or(0),
            crate::backend::looper::PADS
        );
        assert_eq!(
            usize::try_from(PREVIEW_PADS).unwrap_or(0),
            crate::backend::bass::PADS
        );
        assert_eq!(
            usize::try_from(MENU_ITEMS).unwrap_or(0),
            crate::menu::MENU_ROWS
        );
    }
}
