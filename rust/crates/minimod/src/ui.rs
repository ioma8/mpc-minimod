//! The keys the user is holding and what the page shows because of it.
//!
//! The button entry point keeps this state; the feature reads it through the
//! module's accessors.

use crate::backend::looper;
use crate::buttons::{self, Action, Context, Held, Modals, OctaveKeys, Screens};
use crate::page;
use crate::paint;
use crate::paint::show_toast;
use mpc_sample_sdk::state::Global;
use mpc_sample_sdk::{ButtonEvent, ButtonFlow};

/// The app's CHOP key.
pub const CHOP_KEY: u8 = 0x4b;
/// A full press, as the app reports it.
const PRESSED: u32 = 0x7f;
/// PLUS.
const KEY_PLUS: u32 = 0x36;
/// The state the button entry point keeps.
#[derive(Clone, Copy, Debug)]
pub struct State {
    /// SHIFT, tracked while the looper owns the pads.
    pub shift: i32,
    /// ERASE, likewise.
    pub erase: i32,
    /// Which press of the trim cycle this is.
    pub trim_step: i32,
    /// Whether the trim key went down already.
    pub b3_down: i32,
    /// PLUS held.
    pub plus_held: i32,
    /// MINUS held.
    pub minus_held: i32,
    /// The 16 Levels octave shift.
    pub octave: i32,
    /// The first trim edge drawn last, or -1.
    pub mark_v0: f32,
    /// The second.
    pub mark_v1: f32,
    /// The object of the last pad press.
    pub last_press_obj: usize,
    /// The last button event, replayed to put the chop mode back.
    pub event: [u8; 16],
    /// Whether `event` holds a real press.
    pub event_valid: i32,
}

impl State {
    /// A fresh state: nothing held, no marks drawn.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            shift: 0,
            erase: 0,
            trim_step: 0,
            b3_down: 0,
            plus_held: 0,
            minus_held: 0,
            octave: 0,
            mark_v0: -1.0,
            mark_v1: -1.0,
            last_press_obj: 0,
            event: [0; 16],
            event_valid: 0,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// The feature callbacks the button ladder uses.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Toggle 16 Levels and keep the MIDI dispatcher in step.
    pub toggle_levels: Option<fn()>,
    /// Open the effects menu.
    pub menu_open: Option<fn()>,
    /// Close it.
    pub menu_close: Option<fn()>,
    /// Run the selected row.
    pub menu_activate: Option<fn()>,
    /// Rename the soft key.
    pub filter_text: Option<fn()>,
    /// Keep the MIDI dispatcher in step with the visible mode.
    pub midi_sync: Option<fn(i32)>,
    /// Whether the 16 Levels screen is up.
    pub levels_active: Option<fn() -> i32>,
    /// Whether the Sample page is visible.
    pub page_visible: Option<fn() -> bool>,
    /// Whether the effects menu is open.
    pub menu_open_state: Option<fn() -> bool>,
    /// Whether the bass control page is open.
    pub bass_edit_state: Option<fn() -> bool>,
    /// Whether a job is running.
    pub busy_state: Option<fn() -> bool>,
    /// Whether a spectral chop is running.
    pub spectral_state: Option<fn() -> bool>,
    /// Put the chop mode back.
    pub restore_chop: Option<fn(&State)>,
    /// Redraw the menu.
    pub repaint: Option<fn()>,
    /// Drop the trim edges the mod drew.
    pub clear_marks: Option<fn()>,
    /// Release every pad the app may think is held.
    pub release_held: Option<fn()>,
}

/// One button event, handled.
pub fn press(state: &mut State, event: &ButtonEvent, hooks: Hooks, data_down: &mut i32) -> i32 {
    state.shift = i32::from(event.shift);
    state.erase = i32::from(event.erase);
    let code = event.code;
    let down = event.value;
    let pressed = down == PRESSED;
    let levels = hooks.levels_active.map_or(0, |callback| callback());
    if let Some(sync) = hooks.midi_sync {
        sync(levels);
    }
    let context = Context {
        code,
        down,
        held: Held {
            shift: state.shift != 0,
        },
        octave: OctaveKeys {
            plus: state.plus_held != 0,
            minus: state.minus_held != 0,
        },
        screens: Screens {
            modals: Modals {
                menu: hooks.menu_open_state.is_some_and(|callback| callback()),
                spectral: hooks.spectral_state.is_some_and(|callback| callback()),
            },
            looper: looper::looper_is_open() != 0,
            levels: levels != 0,
        },
        page_visible: hooks.page_visible.is_some_and(|callback| callback()),
    };
    let action = buttons::dispatch(context);
    if context.screens.looper {
        match action {
            Action::Shift(on) => state.shift = i32::from(on),
            Action::Erase(on) => state.erase = i32::from(on),
            Action::Looper => {
                let was = looper::looper_is_open() != 0;
                let _ = looper::looper_button(
                    code,
                    i32::try_from(down).unwrap_or(0),
                    state.shift,
                    state.erase,
                );
                if was && looper::looper_is_open() == 0 {
                    state.shift = 0;
                    state.erase = 0;
                    if let Some(release) = hooks.release_held {
                        release();
                    }
                    if let Some(repaint) = hooks.repaint {
                        repaint();
                    }
                }
            }
            _ => {}
        }
        return -2;
    }
    // Remember the event: the chop mode is put back by replaying it, and the
    // encoder's surface is reachable through the context it points at.
    remember(state, event.bytes);
    apply(
        state,
        Event {
            code,
            pressed,
            action,
            context: &context,
            hooks,
            data_down,
        },
    );
    kind(action)
}

/// The action as a number, for the caller's log.
const fn kind(action: Action) -> i32 {
    match action {
        Action::Forward => 0,
        Action::Consume => 1,
        Action::Shift(_) => 2,
        Action::Erase(_) => 3,
        Action::Looper => 4,
        Action::MenuOpen => 5,
        Action::MenuClose => 6,
        Action::MenuActivate => 7,
        Action::Autotrim => 8,
        Action::BumpOctave(_) => 9,
        Action::ToggleLevels => 10,
        Action::MenuEncoder => 11,
        Action::ResetPitch => 12,
    }
}

/// One event, as the decision saw it.
struct Event<'a> {
    code: u32,
    pressed: bool,
    action: Action,
    context: &'a Context,
    hooks: Hooks,
    data_down: &'a mut i32,
}

/// Apply one decided action.
fn apply(state: &mut State, event: Event<'_>) {
    let Event {
        code,
        pressed,
        action,
        context,
        hooks,
        data_down,
    } = event;
    let _ = context;
    match action {
        Action::MenuOpen | Action::MenuClose | Action::MenuActivate => {
            *data_down = i32::from(pressed);
            menu(action, hooks);
        }
        Action::MenuEncoder | Action::Consume => {
            if code == 0x6f {
                *data_down = i32::from(pressed);
            }
        }
        Action::Shift(on) => {
            state.shift = i32::from(on);
            if !pressed {
                state.trim_step = 0;
                state.b3_down = 0;
                if let Some(restore) = hooks.restore_chop {
                    restore(state);
                }
            }
            if let Some(text) = hooks.filter_text {
                text();
            }
        }
        Action::Autotrim => {
            if pressed && state.b3_down == 0 {
                crate::autotrim::run(crate::autotrim::Context {
                    surface: crate::control::surface(),
                    control: crate::control::control(),
                    step: state.trim_step,
                    last_pad: crate::input::last_pad(),
                });
                state.trim_step = (state.trim_step + 1) % 4;
            }
            state.b3_down = i32::from(pressed);
        }
        Action::BumpOctave(steps) => {
            if code == KEY_PLUS {
                state.plus_held = i32::from(pressed);
            } else {
                state.minus_held = i32::from(pressed);
            }
            if let Some((next, text)) = crate::levels::bump(state.octave, steps) {
                state.octave = next;
                show_toast(&text);
            }
        }
        Action::ResetPitch => {
            state.octave = 0;
            show_toast("Pitch reset");
        }
        Action::ToggleLevels => {
            if let Some(toggle) = hooks.toggle_levels {
                toggle();
            }
        }
        Action::Forward => {
            if pressed && state.mark_v0 >= 0.0 {
                state.mark_v0 = -1.0;
                state.mark_v1 = -1.0;
                if let Some(clear) = hooks.clear_marks {
                    clear();
                }
            }
        }
        Action::Erase(_) | Action::Looper => {}
    }
}

/// Run the selected menu row.
fn menu(action: Action, hooks: Hooks) {
    let callback = match action {
        Action::MenuOpen => hooks.menu_open,
        Action::MenuClose => hooks.menu_close,
        _ => hooks.menu_activate,
    };
    if let Some(callback) = callback {
        callback();
    }
}

/// Store the event and derive what the encoder hook needs from it.
fn remember(state: &mut State, bytes: [u8; 16]) {
    state.event = bytes;
    state.event_valid = 1;
}

/// The 16 Levels octave shift.
#[must_use]
pub fn octave() -> i32 {
    state().octave
}

/// Move the 16 Levels octave shift.
pub fn set_octave(value: i32) {
    state_mut().octave = value;
}

/// Which press of the trim cycle this is.
#[must_use]
pub fn trim_step() -> i32 {
    state().trim_step
}

/// Whether SHIFT is held.
#[must_use]
pub fn shift() -> i32 {
    state().shift
}

/// Whether ERASE is held.
#[must_use]
pub fn erase() -> i32 {
    state().erase
}

/// The state itself.
static STATE: Global<State> = Global::new(State::new());

/// The state, for the feature code.
#[must_use]
pub fn state() -> &'static State {
    crate::ui_read(&STATE)
}

/// The state, to change.
#[must_use]
pub fn state_mut() -> &'static mut State {
    crate::ui_write(&STATE)
}

/// The trim marks the page draws.
#[must_use]
pub fn marks() -> [f32; 2] {
    let state = state();
    [state.mark_v0, state.mark_v1]
}

/// The menu's encoder-down flag, which the button hook tracks.
static DATA_DOWN: Global<i32> = Global::new(0);

/// The event the page last sent, copied.
#[must_use]
pub fn event_copy() -> [u8; 16] {
    state().event
}

/// The chop mode the page was in before the mod armed an extraction.
static CHOP_BEFORE: Global<i32> = Global::new(0);

/// The chop mode before the last extraction.
#[must_use]
pub fn chop_before() -> i32 {
    *crate::ui_read(&CHOP_BEFORE)
}

/// Put the chop mode back by replaying a CHOP press, when that is safe.
pub fn minimod_restore_chop(state: &State) {
    let screen = paint::screen();
    // SAFETY: `screen` is the live component recorded by the paint hook on this
    // UI thread.
    let visible = unsafe { page::visible(screen) };
    let (valid, shifted) = (state.event_valid, state.shift);
    if valid == 0 || shifted != 0 || !visible {
        crate::menu::state_mut().restore_pending = 1;
        return;
    }
    // SAFETY: the same live page as above.
    let mode = unsafe { page::chop_mode(screen) };
    if i32::from(mode) == chop_before() {
        return;
    }
    // SAFETY: `state.event` was captured by the button hook in this UI event
    // path and its surface is still live.
    if !unsafe { mpc_sample_sdk::sites::replay_button(&state.event, CHOP_KEY) } {
        crate::menu::state_mut().restore_pending = 1;
    }
}

/// Repaint the component the page last painted.
pub fn minimod_repaint_screen() {
    mpc_sample_sdk::draw::repaint_painted();
}

/// Whether the Sample page is showing.
#[must_use]
pub fn screen_visible() -> bool {
    // SAFETY: the paint hook recorded this component on this UI thread.
    unsafe { page::visible(paint::screen()) }
}

/// Whether the page is in chop mode.
#[must_use]
pub fn screen_chop_mode() -> bool {
    // SAFETY: the paint hook recorded this component on this UI thread.
    unsafe { page::chop_mode(paint::screen()) }
}

/// Drop the trim edges the mod drew.
pub fn clear_marks() {
    minimod_repaint_screen();
}

/// Whether a spectral chop is running.
#[must_use]
pub fn spectral_state() -> bool {
    crate::ui_read(&crate::spectral::G_SPECTRAL).active == 1
}

/// The feature callbacks the button ladder uses.
pub fn hooks() -> Hooks {
    Hooks {
        toggle_levels: Some(crate::midi::toggle_levels),
        menu_open: Some(crate::menu::open_menu),
        menu_close: Some(crate::menu::close_menu),
        menu_activate: Some(crate::jobflow::menu_activate),
        filter_text: Some(crate::menu::filter_text),
        midi_sync: Some(crate::midi::set_on),
        levels_active: Some(crate::midi::levels_active),
        page_visible: Some(screen_visible),
        menu_open_state: Some(crate::menu::is_open),
        bass_edit_state: Some(crate::menu::is_bass_edit),
        busy_state: Some(crate::menu::is_busy),
        spectral_state: Some(spectral_state),
        restore_chop: Some(minimod_restore_chop),
        repaint: Some(minimod_repaint_screen),
        clear_marks: Some(clear_marks),
        release_held: Some(crate::input::release_held),
    }
}

/// The button entry: run the ladder and tell the SDK whether to forward.
#[must_use]
pub fn button(event: ButtonEvent) -> ButtonFlow {
    let hooks = hooks();
    let state = state_mut();
    let data_down = crate::ui_write(&DATA_DOWN);
    if event.surface != 0 {
        crate::input::set_data_target(mpc_sample_sdk::event::data_target(event.surface));
    }
    let kind = press(state, &event, hooks, data_down);
    // SHIFT and 16 LEVELS are the app's own keys: the mod tracks the mode, but
    // the app must still see the press and release or it never toggles its UI.
    if forwards(kind) {
        ButtonFlow::Forward
    } else {
        ButtonFlow::Consume
    }
}

/// Whether an action kind must also reach the app.
const fn forwards(kind: i32) -> bool {
    matches!(kind, 0 | 2 | 10)
}

/// The soft key's label: rename `Filter` to `AutoTrim` while SHIFT is held.
#[must_use]
pub fn label(label: &core::ffi::CStr) -> Option<&'static core::ffi::CStr> {
    if label.to_bytes() == b"Filter" && shift() != 0 {
        Some(c"AutoTrim")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn a_fresh_state_holds_nothing_and_draws_no_marks() {
        let state = State::new();
        assert_eq!(state.shift, 0);
        assert_eq!(state.erase, 0);
        assert_eq!(state.trim_step, 0);
        assert!(state.mark_v0 < 0.0 && state.mark_v1 < 0.0);
        assert_eq!(state.event.len(), 16);
    }
}

#[cfg(test)]
mod forwarding_tests {
    use super::forwards;

    #[test]
    fn the_app_keeps_shift_and_16_levels() {
        assert!(forwards(0), "forward");
        assert!(forwards(2), "SHIFT");
        assert!(forwards(10), "16 LEVELS");
        assert!(!forwards(9), "mod-owned octave bump");
    }
}
