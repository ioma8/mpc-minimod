//! The spectral chop's place in the frame.
//!
//! The input hook only raises a request; the work happens from the paint hook,
//! on the UI thread, after the app has drawn.  That is why this is a small state
//! machine rather than a task: one step per frame, three seconds at most.

use crate::menu;
use crate::paint;
use mpc_sample_sdk::state::Global;

/// How long a run may take before it gives up.
const DEADLINE_MS: i64 = 3_000;
/// How long a claim may wait for the page to start it.
const CLAIM_MS: i64 = 5_000;

/// What the chop is doing.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// 1 while a run is in flight, -1 while it is being claimed.
    pub active: i32,
    /// 0 before the sample is read, 1 afterwards.
    pub phase: i32,
    /// How many slices the pad ended up with.
    pub slices: i32,
    /// When the run gives up, in milliseconds.
    pub deadline: i64,
    /// The button event that started it.
    pub event: [u8; 16],
    /// The screen it started on.
    pub screen: usize,
    /// The page state it started with.
    pub page_state: usize,
    /// The sample it is chopping.
    pub sample: usize,
    /// Its frame count.
    pub frames: u32,
    /// Its channel count.
    pub channels: u32,
    /// Its rate.
    pub rate: u32,
}

/// The feature callbacks the chop uses.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Replace the status line.
    pub set_status: Option<fn(&str)>,
    /// Redraw the screen.
    pub repaint: Option<fn()>,
}

/// Claim a run; false when one is already going or a job is busy.
pub fn start(state: &mut State, event: &[u8], busy: bool) -> bool {
    if busy || state.active != 0 {
        return false;
    }
    state.active = 1;
    state.phase = 0;
    state.screen = 0;
    state.slices = 0;
    // The claim expires if the page never paints to start the run.
    state.deadline = crate::clock::now_ms() + CLAIM_MS;
    state.event = [0; 16];
    let length = event.len().min(state.event.len());
    state.event[..length].copy_from_slice(&event[..length]);
    true
}

/// Whether a claim never reached the page before its deadline.
///
/// A claim is picked up by the paint hook, and a page that never paints produces
/// no tick at all; this is therefore checked from the input hooks, which run
/// whether or not the Sample page draws.
#[must_use]
pub fn claim_expired(state: &State, now: i64) -> bool {
    state.active == 1 && state.phase == 0 && now > state.deadline
}

/// Abandon a run and put the flags back.
pub fn finish(state: &mut State, menu_state: &mut menu::State, hooks: Hooks, error: Option<&str>) {
    let message = if let Some(error) = error {
        format!("Spectral: {error}")
    } else {
        let noun = if state.slices == 1 { "slice" } else { "slices" };
        format!("Spectral: {} {noun} on this pad", state.slices)
    };
    state.phase = 0;
    state.active = 0;
    menu_state.busy = 0;
    if error.is_some() {
        // A failure must be visible: the app's status line and the toast are
        // drawn behind the open menu, so close the menu before reporting.
        menu_state.open = 0;
        menu_state.bass_edit = 0;
        menu_state.data_down = 0;
    }
    if let Some(status) = hooks.set_status {
        status(&message);
    }
    paint::show_toast(if error.is_some() {
        &message
    } else {
        "Spectral: chopped"
    });
    repaint(hooks);
}

/// Redraw.
fn repaint(hooks: Hooks) {
    if let Some(repaint) = hooks.repaint {
        repaint();
    }
}

/// Run one frame of the chop, on the UI thread.
pub fn tick(state: &mut State, menu_state: &mut menu::State, screen: usize, hooks: Hooks) {
    if state.active != 1 {
        return;
    }
    if state.phase == 0 {
        start_run(state, menu_state, screen, hooks, crate::clock::now_ms());
        return;
    }
    let now = crate::clock::now_ms();
    match crate::spectral_machine::step(now) {
        crate::spectral_machine::Step::Done(slices) => {
            state.slices = i32::try_from(slices).unwrap_or(0);
            finish(state, menu_state, hooks, None);
        }
        crate::spectral_machine::Step::Failed(message) => {
            finish(state, menu_state, hooks, Some(message));
        }
        _ => repaint(hooks),
    }
}

/// Read the sample, hand it to the machine and close the menu.
fn start_run(
    state: &mut State,
    menu_state: &mut menu::State,
    screen: usize,
    hooks: Hooks,
    now: i64,
) {
    // SAFETY: `screen` is the live Sample page recorded by the paint hook.
    let Some(selected) = (unsafe { crate::selection::current(screen) }) else {
        finish(state, menu_state, hooks, Some("no sample on this pad"));
        return;
    };
    let samples = crate::sample::memory(selected.sample())
        .filter(|memory| {
            memory.frames == selected.frames()
                && memory.channels == selected.channels()
                && memory.rate == selected.rate()
        })
        .and_then(|memory| memory.read())
        .or_else(|| {
            let path = crate::paths::for_sample(
                selected.sample(),
                selected.frames(),
                selected.channels(),
                selected.rate(),
            )?;
            crate::wav::read_f32(
                &path,
                selected.frames(),
                selected.channels(),
                selected.rate(),
            )
        });
    let Some(samples) = samples else {
        finish(state, menu_state, hooks, Some("cannot read sample audio"));
        return;
    };
    let event = state.event;
    match crate::spectral_machine::begin(now, &selected, &samples, Some(&event)) {
        crate::spectral_machine::Step::Started => {
            menu_state.open = 0;
            menu_state.busy = 1;
            state.phase = 1;
            state.screen = selected.screen();
            state.page_state = selected.state();
            state.sample = selected.sample();
            state.frames = selected.frames();
            state.channels = u32::from(selected.channels());
            state.rate = selected.rate();
            state.deadline = now + DEADLINE_MS;
            repaint(hooks);
        }
        crate::spectral_machine::Step::Failed(message) => {
            finish(state, menu_state, hooks, Some(message));
        }
        _ => {}
    }
}

/// End a run whose page went away.  It is called from the input hooks, which
/// run whether or not the Sample page paints.
///
/// # Safety
///
/// Reads the app's page objects; the state is the crate's own.
pub fn cancel_if_page_gone() {
    let state = crate::ui_write(&G_SPECTRAL);
    if state.active != 1 {
        return;
    }
    let hooks = Hooks {
        set_status: Some(crate::status::set_status),
        repaint: Some(crate::ui::minimod_repaint_screen),
    };
    // A claim that never reached the page must not mute every pad for good.  It
    // cannot be caught in `tick` (a page that never paints never ticks), so the
    // input hooks check it here.
    if claim_expired(state, crate::clock::now_ms()) {
        finish(state, menu::state_mut(), hooks, Some("timed out"));
        return;
    }
    let screen = paint::screen();
    // SAFETY: `screen` is the live component recorded by the paint hook.
    let visible = unsafe { crate::page::visible(screen) };
    if visible && (state.screen == 0 || screen == state.screen) {
        return;
    }
    finish(state, menu::state_mut(), hooks, Some("page changed"));
}

/// The spectral run.
pub(crate) static G_SPECTRAL: Global<State> = Global::new(State {
    active: 0,
    phase: 0,
    slices: 0,
    deadline: 0,
    event: [0; 16],
    screen: 0,
    page_state: 0,
    sample: 0,
    frames: 0,
    channels: 0,
    rate: 0,
});

/// One frame of the chop's state machine, driven from the paint.
pub fn frame(screen: usize) {
    let hooks = Hooks {
        set_status: Some(crate::status::set_status),
        repaint: Some(crate::ui::minimod_repaint_screen),
    };
    tick(
        crate::ui_write(&G_SPECTRAL),
        menu::state_mut(),
        screen,
        hooks,
    );
}

/// Start a chop from the crate's own record and the event that started it.
#[must_use]
pub fn run_start(busy: bool) -> bool {
    let event = crate::ui::event_copy();
    start(crate::ui_write(&G_SPECTRAL), &event, busy)
}

#[cfg(test)]
mod tests {
    use super::{State, claim_expired, start};

    #[test]
    fn a_run_cannot_start_twice() {
        let mut state = State::default();
        assert!(start(&mut state, &[1, 2, 3], false));
        assert!(!start(&mut state, &[4], false), "already running");
        let mut idle = State::default();
        assert!(!start(&mut idle, &[1], true), "job busy");
    }

    #[test]
    fn the_event_is_copied_in() {
        let mut state = State::default();
        assert!(start(&mut state, &[9, 8, 7], false));
        assert_eq!(&state.event[..3], &[9, 8, 7]);
        assert_eq!(state.event.len(), 16);
    }

    #[test]
    fn a_claim_that_is_never_picked_up_expires() {
        let mut state = State::default();
        assert!(start(&mut state, &[1], false));
        assert!(
            !claim_expired(&state, state.deadline),
            "still in the window"
        );
        assert!(claim_expired(&state, state.deadline + 1));
        // Once the page has picked the claim up it is a run, not a claim.
        state.phase = 1;
        assert!(!claim_expired(&state, state.deadline + 1));
    }
}

#[cfg(test)]
mod failure_tests {
    use super::{Hooks, State, finish};
    use crate::menu;

    #[test]
    fn a_failure_closes_the_menu_so_the_message_is_visible() {
        let mut state = State::default();
        let mut menu_state = menu::State {
            open: 1,
            bass_edit: 1,
            ..menu::State::default()
        };
        finish(
            &mut state,
            &mut menu_state,
            Hooks {
                set_status: None,
                repaint: None,
            },
            Some("no onsets"),
        );
        assert_eq!(menu_state.open, 0);
        assert_eq!(menu_state.bass_edit, 0);
        assert_eq!(state.active, 0);
    }
}
