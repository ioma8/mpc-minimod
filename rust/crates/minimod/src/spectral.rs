//! The spectral chop's place in the frame.
//!
//! Sample audio is captured on the UI thread; analysis runs on an SDK worker.
//! The UI then advances the marker machine one step per frame.

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use mpc_sample_sdk::state::Global;
use mpc_sample_sdk::task::{Task, TaskError};

use crate::menu;
use crate::paint;
use crate::selection::{self, Selection};
use crate::spectral_machine::{self, MAX_SLICES};
use crate::status;

/// How long the marker phases may take before the run gives up.
const DEADLINE_MS: i64 = 3_000;
/// How long the worker may take to read and analyse the sample.
const DETECT_TIMEOUT_MS: i64 = 30_000;
/// Native Chop transitions schedule model updates after the view appears.
const CHOP_SETTLE_MS: i64 = 300;
/// Let the firmware complete the 16 Levels exit before opening Chop.
const LEVELS_EXIT_MS: i64 = 150;

/// What the chop is doing.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// 1 while a run is in flight, 0 when idle.
    pub active: i32,
    /// 0 while detecting, 1 while writing, 2 while Chop opens.
    pub phase: i32,
    /// Waiting for the initiating DATA release before opening Chop.
    pub open_pending: bool,
    /// Button release retained while 16 Levels closes.
    pub pending_button: [u8; 16],
    /// Earliest time to replay the saved button.
    pub open_after: i64,
    /// How many slices the pad ended up with.
    pub slices: i32,
    /// When the current phase gives up, in milliseconds.
    pub deadline: i64,
    /// Earliest time the native Chop view may be used.
    pub settle_at: i64,
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
    /// Owned audio captured while the selected sample is live.
    audio: Option<Vec<f32>>,
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
    if state.active == 0 {
        reap_detection();
    }
    if !can_start(state, busy, detection_slot().is_some()) {
        return false;
    }
    // SAFETY: called on the owning UI thread with the live selected page.
    let audio = unsafe { selected.read_current() }.ok();
    state.active = 1;
    state.phase = 0;
    state.open_pending = open_pending;
    state.pending_button = [0; 16];
    state.open_after = 0;
    state.settle_at = 0;
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
            audio,
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

/// Discard an abandoned result only once its worker has finished.
fn reap_detection() {
    let mut slot = detection_slot();
    reap_task(&mut slot);
}

fn reap_task<T>(slot: &mut Option<Task<T>>) {
    if let Some(task) = slot.as_mut()
        && !matches!(task.take(), Err(TaskError::Running))
    {
        *slot = None;
    }
}

fn can_start(state: &State, busy: bool, detection_running: bool) -> bool {
    !busy && state.active == 0 && !detection_running
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
    let Some(samples) = job.audio.take() else {
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
    LIVE_ACTIVE.store(false, Ordering::Release);
    // Dropping a Task does not stop its worker. Keep ownership until terminal.
    reap_detection();
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
    if state.open_after != 0 {
        if crate::clock::now_ms() < state.open_after {
            crate::ui::request_repaint();
            return;
        }
        state.open_after = 0;
        // SAFETY: the saved event belongs to the active Sample page; the SDK
        // checks that its context is still a live native object.
        if !unsafe {
            mpc_sample_sdk::event::replay_button_live(&state.pending_button, crate::ui::CHOP_KEY)
        } {
            finish(state, menu_state, hooks, Some("cannot open Chop"));
            return;
        }
        state.open_pending = false;
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
        state.settle_at = 0;
        if now > state.deadline {
            finish(state, menu_state, hooks, Some("Chop did not open"));
        } else {
            crate::ui::request_repaint();
        }
        return;
    }
    if state.settle_at == 0 {
        state.settle_at = now + CHOP_SETTLE_MS;
    }
    if now < state.settle_at {
        crate::ui::request_repaint();
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

/// Published ownership for live pad events.
static LIVE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether live input must be consumed during a chop.
#[must_use]
pub(crate) fn live_active() -> bool {
    LIVE_ACTIVE.load(Ordering::Acquire)
}

/// The spectral run.
static G_SPECTRAL: Global<State> = Global::new(State {
    active: 0,
    phase: 0,
    open_pending: false,
    pending_button: [0; 16],
    open_after: 0,
    slices: 0,
    deadline: 0,
    settle_at: 0,
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
pub fn run_start(busy: bool, event: &[u8; 16], last_pad: i32) -> bool {
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
                mpc_sample_sdk::event::replay_button(
                    event,
                    u8::try_from(crate::buttons::KEY_LEVELS).unwrap_or(0),
                )
            } {
                status::set_status("Spectral: cannot exit 16 Levels");
                return false;
            }
            crate::midi::toggle_levels(last_pad);
            // Let the firmware finish leaving 16 Levels before opening Chop.
            // DATA's release provides a fresh event without another user press.
            open_pending = true;
        }
        if !open_pending {
            // SAFETY: `event` belongs to the current button callback.
            if !unsafe { mpc_sample_sdk::event::replay_button(event, crate::ui::CHOP_KEY) } {
                status::set_status("Spectral: cannot open Chop");
                return false;
            }
        }
    }
    // SAFETY: `G_SPECTRAL` is owned by the app's UI thread.
    let started =
        unsafe { G_SPECTRAL.with_mut(|state| start(state, selected, busy, open_pending)) };
    LIVE_ACTIVE.store(started, Ordering::Release);
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
    // The press that closed 16 Levels can return before its view has exited.
    // Save the release and replay it after the transition on the UI thread.
    // SAFETY: this state belongs to the current UI callback.
    unsafe {
        G_SPECTRAL.with_mut(|state| {
            state.pending_button = event.bytes;
            state.open_after = crate::clock::now_ms() + LEVELS_EXIT_MS;
        });
    }
    crate::ui::request_repaint();
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
    use super::{Hooks, State, can_start, finish};
    use crate::menu;

    #[test]
    fn cancelled_worker_remains_owned_until_completion() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut slot = Some(mpc_sample_sdk::task::Task::spawn(0, (), move |()| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.recv().unwrap();
        super::reap_task(&mut slot);
        assert!(!can_start(&State::default(), false, slot.is_some()));
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while slot.is_some() {
            super::reap_task(&mut slot);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(can_start(&State::default(), false, slot.is_some()));
    }

    #[test]
    fn a_run_cannot_start_twice() {
        let state = State::default();
        assert!(!can_start(&state, true, false), "job busy");
        assert!(can_start(&state, false, false));
        let state = State {
            active: 1,
            ..State::default()
        };
        assert!(!can_start(&state, false, false), "already running");
        assert!(!can_start(&State::default(), false, true), "worker exists");
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
