//! The keys the user is holding and what the page shows because of it.
//!
//! The button entry point keeps this state; the feature reads it through the
//! module's accessors.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::backend::looper;
use crate::buttons::{self, Action, Context, Held, Modals, OctaveKeys, Screens};
use crate::page;
use crate::paint;
use crate::paint::show_toast;
use mpc_sample_sdk::state::Global;
use mpc_sample_sdk::{ButtonEvent, ButtonFlow};

/// The app's CHOP key.
pub use mpc_sample_sdk::event::button::CHOP as CHOP_KEY;
/// A full press, as the app reports it.
/// PLUS.
use mpc_sample_sdk::event::button::PLUS as KEY_PLUS;
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
    /// The synth preview shift, in eight-semitone steps.
    pub synth_octave: i32,
    /// The current button event, passed to menu actions.
    pub event: [u8; 16],
}

impl State {
    /// A fresh state: nothing held, no button event captured.
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
            synth_octave: 0,
            event: [0; 16],
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// What the SDK should do with a button event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Hand the event to the app.
    Forward,
    /// Swallowed by the mod.
    Consume,
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
    /// Run the selected row, with the event that asked for it.
    pub menu_activate: Option<fn(&[u8; 16])>,
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
    /// Whether a job is running.
    pub busy_state: Option<fn() -> bool>,
    /// Whether a spectral chop is running.
    pub spectral_state: Option<fn() -> bool>,
    /// Redraw the menu.
    pub repaint: Option<fn()>,
    /// Release every pad the app may think is held.
    pub release_held: Option<fn()>,
}

/// One button event, handled.
pub fn press(state: &mut State, event: &ButtonEvent, hooks: Hooks) -> Flow {
    state.shift = i32::from(event.shift);
    state.erase = i32::from(event.erase);
    let code = event.code;
    let down = event.value;
    let pressed = event.pressed;
    let levels = hooks.levels_active.map_or(0, |callback| callback());
    if let Some(sync) = hooks.midi_sync {
        sync(levels);
    }
    let page_visible = hooks.page_visible.is_some_and(|callback| callback());
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
            bass_edit: crate::menu::state().bass_edit != 0,
        },
        page_visible,
        trim_screen: trim_screen(),
        chop_mode: page_visible
            && matches!(code, buttons::KEY_B3 | buttons::KEY_LEVELS)
            && screen_chop_mode(),
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
                    crate::looper_lights::hide();
                    state.shift = 0;
                    state.erase = 0;
                    crate::cc::save();
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
        return Flow::Consume;
    }
    // Menu actions may need the current button event.
    remember(state, event.bytes);
    apply(
        state,
        &Event {
            code,
            pressed,
            action,
            hooks,
        },
    );
    flow(action)
}

/// Whether an action must also reach the app.
const fn flow(action: Action) -> Flow {
    match action {
        Action::Forward | Action::Shift(_) | Action::ToggleLevels => Flow::Forward,
        _ => Flow::Consume,
    }
}

/// One event, as the decision saw it.
#[derive(Clone, Copy)]
struct Event {
    code: u32,
    pressed: bool,
    action: Action,
    hooks: Hooks,
}

/// Apply one decided action.
fn apply(state: &mut State, event: &Event) {
    let Event {
        code,
        pressed,
        action,
        hooks,
    } = *event;
    let bytes = state.event;
    match action {
        Action::MenuOpen | Action::MenuClose | Action::MenuActivate => menu(action, &bytes, hooks),
        Action::Shift(on) => {
            state.shift = i32::from(on);
            if !pressed {
                state.trim_step = 0;
                state.b3_down = 0;
            }
            if let Some(text) = hooks.filter_text {
                text();
            }
        }
        Action::Autotrim => {
            if pressed && state.b3_down == 0 {
                let trimmed = crate::autotrim::run(crate::autotrim::Context {
                    surface: crate::control::surface(),
                    control: crate::control::control(),
                    step: state.trim_step,
                    last_pad: crate::input::last_pad(),
                });
                if trimmed {
                    state.trim_step = (state.trim_step + 1) % 4;
                }
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
            if code == KEY_PLUS {
                state.plus_held = i32::from(pressed);
            } else {
                state.minus_held = i32::from(pressed);
            }
            state.octave = 0;
            show_toast("Pitch reset");
        }
        Action::BumpSynthOctave(steps) => {
            if code == KEY_PLUS {
                state.plus_held = i32::from(pressed);
            } else {
                state.minus_held = i32::from(pressed);
            }
            if let Some((next, text)) = crate::levels::bump(state.synth_octave, steps) {
                state.synth_octave = next;
                crate::backend::bass::preview_set_transpose(
                    next * crate::levels::SEMITONES_PER_STEP,
                );
                show_toast(&text);
                minimod_repaint_screen();
            }
        }
        Action::ResetSynthPitch => {
            if code == KEY_PLUS {
                state.plus_held = i32::from(pressed);
            } else {
                state.minus_held = i32::from(pressed);
            }
            state.synth_octave = 0;
            crate::backend::bass::preview_set_transpose(0);
            show_toast("Pitch reset");
            minimod_repaint_screen();
        }
        Action::ToggleLevels => {
            if let Some(toggle) = hooks.toggle_levels {
                toggle();
            }
        }
        Action::Forward
        | Action::Consume
        | Action::MenuEncoder
        | Action::Erase(_)
        | Action::Looper => {}
    }
}

/// Run the selected menu row.
fn menu(action: Action, event: &[u8; 16], hooks: Hooks) {
    match action {
        Action::MenuOpen => {
            if let Some(callback) = hooks.menu_open {
                callback();
            }
        }
        Action::MenuClose => {
            if let Some(callback) = hooks.menu_close {
                callback();
            }
        }
        Action::MenuActivate => {
            if let Some(callback) = hooks.menu_activate {
                callback(event);
            }
        }
        _ => {}
    }
}

/// Store the current button event for menu actions.
fn remember(state: &mut State, bytes: [u8; 16]) {
    state.event = bytes;
}

/// The 16 Levels octave shift.
#[must_use]
pub fn octave() -> i32 {
    state().octave
}

/// Reset the synth preview shift when its page opens.
pub fn reset_synth_octave() {
    with_state_mut(|state| state.synth_octave = 0);
    crate::backend::bass::preview_reset_note();
}

/// Whether SHIFT is held.
#[must_use]
pub fn shift() -> i32 {
    state().shift
}

/// Whether the app is showing the trim screen.
#[must_use]
pub fn trim_screen() -> bool {
    TRIM_SCREEN.load(Ordering::Acquire)
}

/// The state itself.
static STATE: Global<State> = Global::new(State::new());
/// Whether the app is showing the trim screen (its B3 label is `Filter`).
static TRIM_SCREEN: AtomicBool = AtomicBool::new(false);

/// A copy of the state, for the feature code.
#[must_use]
pub fn state() -> State {
    // SAFETY: `STATE` is owned by the app's UI thread and this copy does not
    // outlive the access.
    unsafe { STATE.with(|state| *state) }
}

/// Run `f` with the state, to change it.
pub fn with_state_mut<R>(f: impl FnOnce(&mut State) -> R) -> R {
    // SAFETY: `STATE` is owned by the app's UI thread; the SDK serialises the
    // callbacks and the closure must not take another borrow of `STATE`.
    unsafe { STATE.with_mut(f) }
}

/// A changed chop view may only be restored with a fresh, unshifted button event.
static CHOP_RESTORE: Global<Option<(usize, bool)>> = Global::new(None);

/// Remember a chop view changed by an extraction until the next user input.
pub fn queue_chop_restore(screen: usize, before: bool, after: bool) {
    // SAFETY: the pending restoration belongs to the UI thread.
    unsafe {
        CHOP_RESTORE.with_mut(|pending| *pending = (before != after).then_some((screen, before)));
    };
}

/// Discard a pending restoration when the user takes control of the page.
pub fn cancel_chop_restore() {
    // SAFETY: input callbacks and the restoration run on the UI thread.
    unsafe { CHOP_RESTORE.with_mut(|pending| *pending = None) };
}

fn restore_pending_chop(event: &ButtonEvent) {
    // SAFETY: the pending restoration belongs to the UI thread.
    let pending = unsafe { CHOP_RESTORE.with_mut(Option::take) };
    let Some((screen, before)) = pending else {
        return;
    };
    if !may_restore_chop(event.shift, event.code, screen, paint::screen()) {
        return;
    }
    // SAFETY: the current callback and page supply live native objects.
    if unsafe { page::visible(screen) && page::chop_mode(screen) != before } {
        // SAFETY: these bytes belong to the current button callback.
        let _ = unsafe { mpc_sample_sdk::sites::replay_button(&event.bytes, CHOP_KEY) };
    }
}

fn may_restore_chop(shift: bool, code: u32, original_screen: usize, screen: usize) -> bool {
    !shift && code != buttons::KEY_SHIFT && code != u32::from(CHOP_KEY) && screen == original_screen
}

/// Repaint the component the page last painted.
pub fn minimod_repaint_screen() {
    mpc_sample_sdk::draw::repaint_painted();
}

/// Set from any thread to ask the repaint pump for one frame.
static REPAINT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ask for one frame without calling into the app: safe from a worker thread.
pub fn request_repaint() {
    REPAINT_REQUESTED.store(true, Ordering::Release);
}

/// The component the repaint pump should ask for, or zero when the view is
/// idle.
///
/// The looper needs a steady 60 ms cadence for its pulse bar; everything else
/// (a finished render, a spectral step) only needs one frame, requested with
/// [`request_repaint`].  This runs on the SDK's pump thread, so it only reads
/// atomics.
#[must_use]
pub fn pump_component() -> usize {
    if looper::looper_is_open() != 0 {
        return current_component();
    }
    if REPAINT_REQUESTED.swap(false, Ordering::AcqRel) {
        current_component()
    } else {
        0
    }
}

/// The component the page last painted, or the Sample page before the first
/// paint.
fn current_component() -> usize {
    match mpc_sample_sdk::draw::painted() {
        0 => page::screen(),
        painted => painted,
    }
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

/// Whether a spectral chop is running.
#[must_use]
pub fn spectral_state() -> bool {
    crate::spectral::is_active()
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
        busy_state: Some(crate::menu::is_busy),
        spectral_state: Some(spectral_state),
        repaint: Some(minimod_repaint_screen),
        release_held: Some(crate::input::release_held),
    }
}

/// The button entry: run the ladder and tell the SDK whether to forward.
#[must_use]
pub fn button(event: ButtonEvent) -> ButtonFlow {
    restore_pending_chop(&event);
    // The first soft key enters the trim screen; SAMPLE and the other screen
    // soft keys leave it. The label `Filter` can also arm the flag.
    if event.value == buttons::PRESSED {
        if event.code == buttons::KEY_SCREEN1 {
            TRIM_SCREEN.store(true, Ordering::Release);
        } else if buttons::leaves_trim_screen(event.code) {
            TRIM_SCREEN.store(false, Ordering::Release);
        }
    }
    let hooks = hooks();
    if event.surface != 0 {
        crate::input::set_data_target(mpc_sample_sdk::event::data_target(event.surface));
    }
    // SHIFT and 16 LEVELS are the app's own keys: the mod tracks the mode, but
    // the app must still see the press and release or it never toggles its UI.
    with_state_mut(|state| match press(state, &event, hooks) {
        Flow::Forward => ButtonFlow::Forward,
        Flow::Consume => ButtonFlow::Consume,
    })
}

/// The soft key's label: rename `Filter` to `AutoTrim` while SHIFT is held.
#[must_use]
pub fn label(label: &core::ffi::CStr) -> Option<&'static core::ffi::CStr> {
    if label.to_bytes() == b"Filter" {
        TRIM_SCREEN.store(true, Ordering::Release);
    }
    if label.to_bytes() == b"Filter" && shift() != 0 {
        Some(c"AutoTrim")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHOP_KEY, CHOP_RESTORE, State, cancel_chop_restore, may_restore_chop, queue_chop_restore,
    };

    #[test]
    fn a_fresh_state_holds_nothing() {
        let state = State::new();
        assert_eq!(state.shift, 0);
        assert_eq!(state.erase, 0);
        assert_eq!(state.trim_step, 0);
        assert_eq!(state.event.len(), 16);
    }

    #[test]
    fn chop_restore_is_scoped_to_a_changed_extraction_and_safe_input() {
        queue_chop_restore(123, false, false);
        // SAFETY: this test is the only test accessing the pending restoration.
        assert_eq!(unsafe { CHOP_RESTORE.with(|pending| *pending) }, None);
        queue_chop_restore(123, false, true);
        // SAFETY: this test exclusively accesses the pending restoration.
        let pending = unsafe { CHOP_RESTORE.with(|pending| *pending) };
        assert_eq!(pending, Some((123, false)));
        assert!(!may_restore_chop(true, 42, 123, 123));
        assert!(!may_restore_chop(
            false,
            super::buttons::KEY_SHIFT,
            123,
            123
        ));
        assert!(!may_restore_chop(false, u32::from(CHOP_KEY), 123, 123));
        assert!(!may_restore_chop(false, 42, 123, 456));
        assert!(may_restore_chop(false, 42, 123, 123));
        cancel_chop_restore();
        // SAFETY: this test exclusively accesses the pending restoration.
        assert_eq!(unsafe { CHOP_RESTORE.with(|pending| *pending) }, None);
    }
}

#[cfg(test)]
mod forwarding_tests {
    use super::{Action, Flow, flow};

    #[test]
    fn the_app_keeps_shift_and_16_levels() {
        assert_eq!(flow(Action::Forward), Flow::Forward);
        assert_eq!(flow(Action::Shift(true)), Flow::Forward);
        assert_eq!(flow(Action::ToggleLevels), Flow::Forward);
        assert_eq!(flow(Action::BumpOctave(1)), Flow::Consume);
        assert_eq!(flow(Action::BumpSynthOctave(1)), Flow::Consume);
        assert_eq!(flow(Action::ResetSynthPitch), Flow::Consume);
        assert_eq!(flow(Action::Autotrim), Flow::Consume);
    }
}
