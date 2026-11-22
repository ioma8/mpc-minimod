//! The keys the user is holding and what the page shows because of it.
//!
//! The button entry point keeps this state; the feature reads it through the
//! module's accessors.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::backend::looper;
use crate::buttons::{self, Action};
use crate::page;
use crate::paint;
use crate::paint::show_toast;
use mpc_sample_sdk::ButtonEvent;
use mpc_sample_sdk::state::Global;

/// The app's CHOP key.
pub use mpc_sample_sdk::event::button::CHOP as CHOP_KEY;
/// A full press, as the app reports it.
/// PLUS.
/// The state the button entry point keeps.
#[derive(Clone, Copy, Debug)]
pub struct State {
    /// Which press of the trim cycle this is.
    pub trim_step: i32,
    /// The 16 Levels octave shift.
    pub octave: i32,
    /// The synth preview shift, in eight-semitone steps.
    pub synth_octave: i32,
    /// Whether the 16 Levels grid is halved (microtonal).
    pub microtonal: bool,
}

impl State {
    /// A fresh state: nothing held, no button event captured.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            trim_step: 0,
            octave: 0,
            synth_octave: 0,
            microtonal: false,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply the feature action selected by the input policy.
///
/// Returns true when closing the Live Looper also requires releasing app-owned
/// pads. The caller performs those releases after this state borrow ends.
pub(crate) fn apply_action(action: Action, event: ButtonEvent, last_pad: i32) -> bool {
    let mut release_pads = false;
    with_state_mut(|state| {
        if looper::looper_is_open() != 0 {
            if let Action::Looper = action {
                let was = looper::looper_is_open() != 0;
                let _ = looper::looper_button(
                    event.code,
                    i32::try_from(event.value).unwrap_or(0),
                    i32::from(event.shift),
                    i32::from(event.erase),
                );
                if was && looper::looper_is_open() == 0 {
                    crate::looper_lights::hide();
                    crate::settings::save(looper::looper_master());
                    minimod_repaint_screen();
                    release_pads = true;
                }
            }
            return;
        }
        match action {
            Action::MenuOpen => crate::menu::open_menu(),
            Action::MenuClose => crate::menu::close_menu(),
            Action::MenuActivate => crate::jobflow::menu_activate(&event.bytes, last_pad),
            Action::Shift(on) => {
                if !on {
                    state.trim_step = 0;
                }
                crate::menu::filter_text();
            }
            Action::Autotrim => {
                if crate::autotrim::run(crate::autotrim::Context {
                    surface: crate::control::surface(),
                    control: crate::control::control(),
                    step: state.trim_step,
                    last_pad,
                }) {
                    state.trim_step = (state.trim_step + 1) % 4;
                }
            }
            Action::BumpOctave(steps) => {
                if let Some((next, text)) = crate::levels::bump(state.octave, steps) {
                    state.octave = next;
                    show_toast(&text);
                }
            }
            Action::ResetPitch => {
                state.octave = 0;
                show_toast("Pitch reset");
            }
            Action::BumpSynthOctave(steps) => {
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
                state.synth_octave = 0;
                crate::backend::bass::preview_set_transpose(0);
                show_toast("Pitch reset");
                minimod_repaint_screen();
            }
            Action::ToggleLevels => crate::midi::toggle_levels(last_pad),
            Action::Forward
            | Action::Consume
            | Action::MenuEncoder
            | Action::Erase(_)
            | Action::Looper => {}
        }
    });
    release_pads
}

/// The 16 Levels octave shift.
#[must_use]
pub fn octave() -> i32 {
    state().octave
}

/// Whether the 16 Levels grid plays half semitones.
#[must_use]
pub fn microtonal() -> bool {
    state().microtonal
}

/// Turn the half-semitone grid on or off.
///
/// Returns the toast text when the mode actually changed, so a turn that holds
/// the current mode does not repeat the message.
#[must_use]
pub fn set_microtonal(on: bool) -> Option<&'static str> {
    let mut changed = false;
    with_state_mut(|state| {
        if state.microtonal != on {
            state.microtonal = on;
            changed = true;
        }
    });
    changed.then_some(if on {
        "Microtonal: half steps"
    } else {
        "Semitones"
    })
}

/// Reset the synth preview shift when its page opens.
pub fn reset_synth_octave() {
    with_state_mut(|state| state.synth_octave = 0);
    crate::backend::bass::preview_reset_note();
}

/// Whether SHIFT is held.
#[must_use]
pub fn shift() -> i32 {
    i32::from(mpc_sample_sdk::shift())
}

/// Track navigation to or from the trim page before classifying a button.
pub fn set_trim_screen(trim: bool) {
    TRIM_SCREEN.store(trim, Ordering::Release);
}

/// Whether the app is showing the trim screen.
#[must_use]
pub fn trim_screen() -> bool {
    TRIM_SCREEN.load(Ordering::Acquire)
}

/// The state itself.
static STATE: Global<State> = Global::new(State::new());
/// Whether the app is showing the trim screen (its B3 label is `Filter`).
///
/// The app opens on the trim page, so this starts true; only the page keys move
/// it. Guessing from a label callback alone left the shortcut dead after the
/// app returned to the trim page on its own.
static TRIM_SCREEN: AtomicBool = AtomicBool::new(true);

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

/// A changed chop view is restored on the completion, or by a fresh unshifted
/// button event when the switch had not happened yet.
static CHOP_RESTORE: Global<Option<(usize, bool)>> = Global::new(None);
/// The most recent button event, kept for restoring a page mode after a job.
///
/// The app's button context is a long-lived object; [`restore_chop_now`]
/// re-checks it is still live before replaying, and only on the page the event
/// came from.
static LAST_BUTTON: Global<([u8; 16], usize)> = Global::new(([0; 16], 0));
/// Arm a chop restore for an extraction that started in full mode.
///
/// The app switches the page to chop mode on some extractions. That can happen
/// while the native command runs or just after it returns, so the switch is not
/// sampled here: the restore is armed from the mode the job *started* in, and
/// the live mode is checked again when a safe button event arrives. A job that
/// started in chop mode (the spectral chop) is left alone.
pub fn queue_chop_restore(screen: usize, before: bool) {
    // SAFETY: the pending restoration belongs to the UI thread.
    unsafe {
        CHOP_RESTORE.with_mut(|pending| *pending = (!before).then_some((screen, before)));
    };
}

/// Discard a pending restoration when the user takes control of the page.
pub fn cancel_chop_restore() {
    // SAFETY: input callbacks and the restoration run on the UI thread.
    unsafe { CHOP_RESTORE.with_mut(|pending| *pending = None) };
}

/// Remember this button event for a later page-mode restore.
pub fn note_button(event: &ButtonEvent) {
    // SAFETY: the event and the page recorded by the paint hook are UI-thread
    // state, and this callback owns them.
    unsafe {
        LAST_BUTTON.with_mut(|last| *last = (event.bytes, paint::screen()));
    }
}

/// Restore the previous chop mode with this live, unshifted button event.
pub fn restore_pending_chop(event: &ButtonEvent) {
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
        let _ = unsafe { mpc_sample_sdk::event::replay_button(&event.bytes, CHOP_KEY) };
    }
}

/// Put the chop mode back right after a finished job, without waiting for the
/// next button.
///
/// Runs from the job's completion on the UI thread. The event bytes come from
/// the last button press on this page; the SDK re-checks that their context is
/// still a live app object before replaying, and the page and mode are checked
/// again here.
pub fn restore_chop_now() {
    // SAFETY: the pending restoration is UI-thread state, written only here and
    // in the arming call on the same thread.
    let pending = unsafe { CHOP_RESTORE.with(|pending| *pending) };
    let Some((screen, before)) = pending else {
        return;
    };
    // SAFETY: `screen` is the page the job ran on and this is its UI thread.
    if paint::screen() != screen || !unsafe { page::visible(screen) } {
        return;
    }
    // SAFETY: the page is live on this thread.
    if unsafe { page::chop_mode(screen) } == before {
        // The app has not switched (yet); keep the restore for the next input.
        return;
    }
    // SAFETY: the recorded event is UI-thread state.
    let (bytes, event_screen) = unsafe { LAST_BUTTON.with(|last| *last) };
    if event_screen != screen {
        return;
    }
    // SAFETY: the same UI-thread state, and this is the only consumer.
    let taken = unsafe { CHOP_RESTORE.with_mut(Option::take) };
    debug_assert!(taken.is_some());
    // SAFETY: the bytes came from this page and the SDK proves the event's
    // context is still a live app object before calling the dispatcher.
    let _ = unsafe { mpc_sample_sdk::event::replay_button_live(&bytes, CHOP_KEY) };
}

fn may_restore_chop(shift: bool, code: u32, original_screen: usize, screen: usize) -> bool {
    !shift && code != buttons::KEY_SHIFT && code != u32::from(CHOP_KEY) && screen == original_screen
}

/// Repaint from a native UI callback while the last-painted component is live.
pub fn minimod_repaint_screen() {
    // SAFETY: every caller runs in the app's UI event or paint callback; the
    // currently painted Sample component stays live for the callback.
    unsafe { mpc_sample_sdk::draw::repaint_painted() };
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
    // Jobs and Spectral advance from paint callbacks. Retry while either is
    // active because the app can drop or coalesce a repaint request.
    if crate::jobflow::running() || crate::spectral::live_active() {
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
        assert_eq!(state.trim_step, 0);
        assert_eq!(state.octave, 0);
        assert_eq!(state.synth_octave, 0);
    }

    #[test]
    fn a_job_that_started_in_full_mode_is_restored_without_sampling_the_switch() {
        // An extraction can switch the page after the native call returns, so
        // arming must not depend on observing the change here.
        queue_chop_restore(123, false);
        // SAFETY: this test exclusively accesses the pending restoration.
        let armed = unsafe { CHOP_RESTORE.with(|pending| *pending) };
        assert_eq!(armed, Some((123, false)));
        queue_chop_restore(123, true);
        // SAFETY: this test exclusively accesses the pending restoration.
        let inside_chop = unsafe { CHOP_RESTORE.with(|pending| *pending) };
        assert_eq!(
            inside_chop, None,
            "a job started inside chop mode must not toggle it off"
        );
        queue_chop_restore(123, false);
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
        let cleared = unsafe { CHOP_RESTORE.with(|pending| *pending) };
        assert_eq!(cleared, None);
    }
}
