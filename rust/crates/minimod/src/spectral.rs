//! The spectral chop's place in the frame.
//!
//! Reading and analysing the sample is slow, so it runs on the SDK's worker
//! (`Task`) exactly like a render job.  The UI thread only polls the result and
//! then advances the three-phase marker machine one step per frame.

use std::sync::{Mutex, MutexGuard, PoisonError};

use mpc_sample_sdk::state::Global;
use mpc_sample_sdk::task::{Task, TaskError};

use crate::menu;
use crate::paint;
use crate::paths;
use crate::sample;
use crate::selection::{self, Selection};
use crate::spectral_machine::{self, MAX_SLICES};
use crate::{status, wav};

/// How long the marker phases may take before the run gives up.
const DEADLINE_MS: i64 = 3_000;
/// How long the worker may take to read and analyse the sample.
const DETECT_TIMEOUT_MS: i64 = 30_000;

/// What the chop is doing.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// 1 while a run is in flight, 0 when idle.
    pub active: i32,
    /// 0 while detecting, 1 while writing, 2 while Chop opens.
    pub phase: i32,
    /// Waiting for the initiating DATA release before opening Chop.
    pub open_pending: bool,
    /// How many slices the pad ended up with.
    pub slices: i32,
    /// When the current phase gives up, in milliseconds.
    pub deadline: i64,
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

/// What the detection worker produces.
struct Detection {
    /// The selection the run started from.
    selection: Selection,
    /// The boundaries, when the analysis succeeded.
    positions: Option<Vec<u32>>,
    /// Why it failed, when it did.
    error: Option<&'static str>,
}

/// The detection worker, if one is running or waiting to be polled.
static DETECTION: Mutex<Option<Task<Detection>>> = Mutex::new(None);
static READY: Mutex<Option<Detection>> = Mutex::new(None);

/// Lock the detection slot, recovering from a poisoned mutex.
fn detection_slot() -> MutexGuard<'static, Option<Task<Detection>>> {
    DETECTION.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Claim a run and start its worker.  False when one is already going, a job is
/// busy, or the page has no sample.
pub fn start(state: &mut State, selected: Selection, busy: bool, open_pending: bool) -> bool {
    if busy || state.active != 0 || detection_slot().is_some() {
        return false;
    }
    state.active = 1;
    state.phase = 0;
    state.open_pending = open_pending;
    state.slices = 0;
    state.deadline = crate::clock::now_ms() + DETECT_TIMEOUT_MS;
    state.screen = selected.screen();
    state.page_state = selected.state();
    state.sample = selected.sample();
    state.frames = selected.frames();
    state.channels = u32::from(selected.channels());
    state.rate = selected.rate();
    let task = Task::spawn(
        state.screen,
        Detection {
            selection: selected,
            positions: None,
            error: None,
        },
        |job| {
            analyse(job);
            // The pump turns this into one frame on the UI thread, which polls
            // the result.
            crate::ui::request_repaint();
            Ok(())
        },
    );
    *detection_slot() = Some(task);
    true
}

/// Read the sample and detect its boundaries, on the worker.
///
/// The worker only sees a geometry snapshot; the marker machine re-reads the
/// live selection on the UI thread before writing, so a sample that changed
/// during the analysis is cancelled there instead of chopped.
fn analyse(job: &mut Detection) {
    let selected = job.selection;
    let Some(cells) = usize::try_from(selected.frames())
        .ok()
        .and_then(|frames| frames.checked_mul(usize::from(selected.channels())))
    else {
        job.error = Some("cannot read sample WAV");
        return;
    };
    let Some(samples) = read_samples(&selected) else {
        job.error = Some("cannot read sample audio");
        return;
    };
    if samples.len() != cells {
        job.error = Some("cannot read sample WAV");
        return;
    }
    match crate::backend::spectral::detect(
        &samples,
        usize::from(selected.channels()),
        selected.rate(),
        MAX_SLICES,
    ) {
        Some(positions) if !positions.is_empty() => job.positions = Some(positions),
        _ => job.error = Some("no onsets"),
    }
}

/// The sample's audio: the in-memory buffer first, then the exact WAV.
fn read_samples(selected: &Selection) -> Option<Vec<f32>> {
    sample::memory(selected.sample())
        .filter(|memory| {
            memory.frames == selected.frames()
                && memory.channels == selected.channels()
                && memory.rate == selected.rate()
        })
        .and_then(|memory| memory.read())
        .or_else(|| {
            let path = paths::for_sample(
                selected.sample(),
                selected.frames(),
                selected.channels(),
                selected.rate(),
            )?;
            wav::read_f32(
                &path,
                selected.frames(),
                selected.channels(),
                selected.rate(),
            )
        })
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
    state.open_pending = false;
    state.active = 0;
    *detection_slot() = None;
    *READY.lock().unwrap_or_else(PoisonError::into_inner) = None;
    menu_state.busy = 0;
    if error.is_some() {
        // A failure must be visible: the app's status line and the toast are
        // drawn behind the open menu, so close the menu before reporting.
        menu_state.open = 0;
        menu_state.bass_edit = 0;
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
pub fn tick(state: &mut State, menu_state: &mut menu::State, hooks: Hooks) {
    if state.active != 1 {
        return;
    }
    if state.phase == 0 {
        poll_detection(state, menu_state, hooks);
        return;
    }
    if state.phase == 2 {
        start_ready(state, menu_state, hooks);
        return;
    }
    let now = crate::clock::now_ms();
    match spectral_machine::step(now) {
        spectral_machine::Step::Done(slices) => {
            state.slices = i32::try_from(slices).unwrap_or(0);
            finish(state, menu_state, hooks, None);
        }
        spectral_machine::Step::Failed(message) => {
            finish(state, menu_state, hooks, Some(message));
        }
        _ => crate::ui::request_repaint(),
    }
}

/// Poll the detection worker and, when it is done, write the boundaries.
fn poll_detection(state: &mut State, menu_state: &mut menu::State, hooks: Hooks) {
    let now = crate::clock::now_ms();
    if now > state.deadline {
        finish(state, menu_state, hooks, Some("timed out"));
        return;
    }
    let Some(mut task) = detection_slot().take() else {
        finish(state, menu_state, hooks, Some("cancelled"));
        return;
    };
    match task.take() {
        Err(TaskError::Running) => {
            *detection_slot() = Some(task);
            crate::ui::request_repaint();
        }
        Err(TaskError::AlreadyTaken | TaskError::Panicked | TaskError::Render(_)) => {
            finish(state, menu_state, hooks, Some("processing failed"));
        }
        Ok(job) => {
            if job.positions.is_none() {
                finish(
                    state,
                    menu_state,
                    hooks,
                    Some(job.error.unwrap_or("no onsets")),
                );
                return;
            }
            *READY.lock().unwrap_or_else(PoisonError::into_inner) = Some(job);
            state.phase = 2;
            state.deadline = now + DEADLINE_MS;
            crate::ui::request_repaint();
        }
    }
}

fn start_ready(state: &mut State, menu_state: &mut menu::State, hooks: Hooks) {
    let now = crate::clock::now_ms();
    if state.open_pending {
        state.deadline = now + DEADLINE_MS;
        crate::ui::request_repaint();
        return;
    }
    // SAFETY: the active run keeps the Sample page on its UI thread.
    if !unsafe { crate::page::chop_mode(state.screen) } {
        if now > state.deadline {
            finish(state, menu_state, hooks, Some("Chop did not open"));
        } else {
            crate::ui::request_repaint();
        }
        return;
    }
    let ready = READY.lock().unwrap_or_else(PoisonError::into_inner).take();
    let Some(job) = ready else {
        finish(state, menu_state, hooks, Some("cancelled"));
        return;
    };
    match spectral_machine::begin(now, &job.selection, job.positions.as_deref().unwrap_or(&[])) {
        spectral_machine::Step::Started => {
            menu_state.open = 0;
            menu_state.busy = 1;
            state.phase = 1;
            state.deadline = now + DEADLINE_MS;
            crate::ui::request_repaint();
        }
        spectral_machine::Step::Failed(message) => finish(state, menu_state, hooks, Some(message)),
        _ => {}
    }
}

/// End a run whose page went away.  It is called from the input hooks, which
/// run whether or not the Sample page paints.
pub fn cancel_if_page_gone() {
    let hooks = Hooks {
        set_status: Some(status::set_status),
        repaint: Some(crate::ui::minimod_repaint_screen),
    };
    let screen = paint::screen();
    // SAFETY: `screen` is the live component recorded by the paint hook.
    let visible = unsafe { crate::page::visible(screen) };
    // SAFETY: `G_SPECTRAL` is owned by the app's UI thread; the closure does not
    // take another borrow of it.
    unsafe {
        G_SPECTRAL.with_mut(|state| {
            if state.active != 1 {
                return;
            }
            if visible && (state.screen == 0 || screen == state.screen) {
                return;
            }
            menu::with_state_mut(|menu| finish(state, menu, hooks, Some("page changed")));
        });
    }
}

/// The spectral run.
static G_SPECTRAL: Global<State> = Global::new(State {
    active: 0,
    phase: 0,
    open_pending: false,
    slices: 0,
    deadline: 0,
    screen: 0,
    page_state: 0,
    sample: 0,
    frames: 0,
    channels: 0,
    rate: 0,
});

/// One frame of the chop's state machine, driven from the paint.
pub fn frame() {
    let hooks = Hooks {
        set_status: Some(status::set_status),
        repaint: Some(crate::ui::minimod_repaint_screen),
    };
    // SAFETY: both cells are owned by the app's UI thread; the nested closures
    // borrow different cells.
    unsafe {
        G_SPECTRAL.with_mut(|state| {
            menu::with_state_mut(|menu| tick(state, menu, hooks));
        });
    }
}

/// Start analysing the selected sample.
#[must_use]
pub fn run_start(busy: bool, event: &[u8; 16]) -> bool {
    let screen = paint::screen();
    // SAFETY: `screen` is the live Sample page recorded by the paint hook in
    // this UI callback.
    let Some(selected) = (unsafe { selection::current(screen) }) else {
        status::set_status("Spectral: no sample on this pad");
        paint::show_toast("Spectral: no sample");
        return false;
    };
    if busy || is_active() {
        return false;
    }
    let mut open_pending = false;
    // The firmware opens Chop asynchronously. Send the press only while the
    // initiating DATA event is live, and never while SHIFT is held.
    // SAFETY: the selected page is live on this UI callback.
    if !unsafe { crate::page::chop_mode(screen) } {
        if mpc_sample_sdk::shift() {
            status::set_status("Spectral: release SHIFT first");
            return false;
        }
        if crate::midi::levels_active() != 0 {
            // The firmware will not open Chop while 16 Levels is active.
            // SAFETY: `event` belongs to the current button callback.
            if !unsafe {
                mpc_sample_sdk::sites::replay_button(
                    event,
                    u8::try_from(crate::buttons::KEY_LEVELS).unwrap_or(0),
                )
            } {
                status::set_status("Spectral: cannot exit 16 Levels");
                return false;
            }
            crate::midi::toggle_levels();
            // Let the firmware finish leaving 16 Levels before opening Chop.
            // DATA's release provides a fresh event without another user press.
            open_pending = true;
        }
        if !open_pending {
            // SAFETY: `event` belongs to the current button callback.
            if !unsafe { mpc_sample_sdk::sites::replay_button(event, crate::ui::CHOP_KEY) } {
                status::set_status("Spectral: cannot open Chop");
                return false;
            }
        }
    }
    // SAFETY: `G_SPECTRAL` is owned by the app's UI thread.
    let started =
        unsafe { G_SPECTRAL.with_mut(|state| start(state, selected, busy, open_pending)) };
    if started {
        crate::ui::minimod_repaint_screen();
    }
    started
}

/// Open Chop after the initiating DATA release when 16 Levels had to exit.
pub fn on_button(event: &mpc_sample_sdk::ButtonEvent) {
    if event.code != crate::buttons::KEY_DATA || event.pressed {
        return;
    }
    // SAFETY: this state belongs to the current UI callback.
    let pending = unsafe { G_SPECTRAL.with(|state| state.active == 1 && state.open_pending) };
    if !pending {
        return;
    }
    if event.shift {
        fail_pending("release SHIFT; retry");
        return;
    }
    // SAFETY: the state belongs to this UI callback; no borrow crosses replay.
    unsafe { G_SPECTRAL.with_mut(|state| state.open_pending = false) };
    // SAFETY: these bytes came from this live button callback.
    if unsafe { mpc_sample_sdk::sites::replay_button(&event.bytes, crate::ui::CHOP_KEY) } {
        crate::ui::request_repaint();
    } else {
        fail_pending("cannot open Chop");
    }
}

fn fail_pending(message: &str) {
    let hooks = Hooks {
        set_status: Some(status::set_status),
        repaint: Some(crate::ui::minimod_repaint_screen),
    };
    // SAFETY: both cells belong to this UI callback.
    unsafe {
        G_SPECTRAL.with_mut(|state| {
            menu::with_state_mut(|menu| finish(state, menu, hooks, Some(message)));
        });
    }
}

/// Whether a chop is running.
#[must_use]
pub fn is_active() -> bool {
    // SAFETY: `G_SPECTRAL` is owned by the app's UI thread.
    unsafe { G_SPECTRAL.with(|state| state.active == 1) }
}

#[cfg(test)]
mod tests {
    use super::{Hooks, State, finish, start};
    use crate::menu;
    use crate::selection::Selection;

    fn selection() -> Selection {
        // SAFETY: the value is never dereferenced; the guards below return
        // before a worker is spawned.
        unsafe { Selection::from_parts(1, 2, 3, 4, 2, 44_100) }
    }

    #[test]
    fn a_run_cannot_start_twice() {
        let mut state = State::default();
        assert!(!start(&mut state, selection(), true, false), "job busy");
        state.active = 1;
        assert!(
            !start(&mut state, selection(), false, false),
            "already running"
        );
    }

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
