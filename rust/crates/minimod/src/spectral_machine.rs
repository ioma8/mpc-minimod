//! The Spectral Chop state machine.
//!
//! Chopping the selected sample is a three-phase dance with the app: switch the
//! page to chop mode, write the boundaries through the app's own slice calls,
//! then verify what it kept. The page is a UI object that can change under us
//! between frames, so every phase re-reads the selection and gives up if it
//! moved.

use crate::chop;
use crate::firmware;
use crate::page;
use crate::paths;
use crate::selection;
use crate::wav;
use minimod_dsp::spectral;

/// How many boundaries the app can take.
pub const MAX_SLICES: usize = 16;
/// `[screen + 0x168]` is the chop binding.
const SCREEN_BINDING: usize = 0x168;
/// `[binding + 0x9c]` is the chop type (0 is Manual).
const BINDING_TYPE: usize = 0x9c;

/// What the caller should do after a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do.
    Idle,
    /// The run just started: the menu closes and the busy flag goes up.
    Started,
    /// Keep going next frame.
    Continue,
    /// Finished; `slices` is what the pad now holds.
    Done(u32),
    /// Finished with an error to show in the status line.
    Failed(&'static str),
}

/// The machine's own state.
#[derive(Clone, Copy, Debug)]
struct Machine {
    screen: usize,
    state: usize,
    sample: usize,
    frames: u32,
    channels: u16,
    rate: u32,
    count: u32,
    positions: [u32; MAX_SLICES],
    phase: u8,
    deadline: i64,
}

static mut MACHINE: Option<Machine> = None;

/// Forget the machine; the caller calls this when a run finishes.
pub fn reset() {
    // SAFETY: only the UI thread touches the machine.
    unsafe {
        MACHINE = None;
    }
}

/// Run one frame of the state machine.
///
/// `active` is the caller's flag: 1 means a run was requested.
#[must_use]
pub fn step(
    base: usize,
    now: i64,
    active: i32,
    screen: usize,
    press_chop: Option<unsafe extern "C" fn()>,
) -> Step {
    if active != 1 {
        return Step::Idle;
    }
    // SAFETY: only the UI thread touches the machine.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(MACHINE) };
    match slot.as_mut() {
        None => start(base, now, screen, press_chop, slot),
        Some(running) => advance(base, now, running),
    }
}

/// Phase 0: find the sample, read it, detect the boundaries, open Manual chop.
fn start(
    base: usize,
    now: i64,
    screen: usize,
    press_chop: Option<unsafe extern "C" fn()>,
    slot: &mut Option<Machine>,
) -> Step {
    let Some(selected) = selection::current(base, screen) else {
        return Step::Failed("no sample on this pad");
    };
    let Some(path) = paths::for_sample(
        selected.sample,
        selected.frames,
        selected.channels,
        selected.rate,
    ) else {
        return Step::Failed("cannot read sample WAV");
    };
    let Some(samples) = wav::read_f32(&path, selected.frames, selected.channels, selected.rate)
    else {
        return Step::Failed("cannot read sample WAV");
    };
    let Ok(onsets) = spectral::detect(
        &samples,
        usize::from(selected.channels),
        selected.rate,
        MAX_SLICES,
    ) else {
        return Step::Failed("no onsets");
    };
    let count = onsets.positions.len().min(MAX_SLICES);
    if count < 2 {
        return Step::Failed("no onsets");
    }
    let mut positions = [0_u32; MAX_SLICES];
    for (index, position) in onsets.positions.iter().take(count).enumerate() {
        if *position >= selected.frames || (index > 0 && *position <= positions[index - 1]) {
            return Step::Failed("invalid onsets");
        }
        positions[index] = *position;
    }
    if !page::chop_mode(base, screen)
        && let Some(press) = press_chop
    {
        // SAFETY: the caller's callback replays the CHOP button.
        unsafe { press() };
    }
    if !chop::set_manual(base, screen) {
        return Step::Failed("cannot select Manual chop");
    }
    *slot = Some(Machine {
        screen: selected.screen,
        state: selected.state,
        sample: selected.sample,
        frames: selected.frames,
        channels: selected.channels,
        rate: selected.rate,
        count: u32::try_from(count).unwrap_or(0),
        positions,
        phase: 1,
        deadline: now + 3000,
    });
    Step::Started
}

/// Phases 1 and 2: wait for Manual chop, write the boundaries, verify them.
fn advance(base: usize, now: i64, running: &mut Machine) -> Step {
    let Some(selected) = selection::current(base, running.screen) else {
        return Step::Failed("sample changed; cancelled");
    };
    if selected.screen != running.screen
        || selected.state != running.state
        || selected.sample != running.sample
        || selected.frames != running.frames
        || selected.channels != running.channels
        || selected.rate != running.rate
    {
        return Step::Failed("sample changed; cancelled");
    }
    let kind = firmware::read_pointer(running.screen.wrapping_add(SCREEN_BINDING))
        .and_then(|binding| firmware::read_u32(binding.wrapping_add(BINDING_TYPE)))
        .map_or(-1, u32::cast_signed);
    if !page::chop_mode(base, running.screen) || kind != 0 {
        return if now >= running.deadline {
            Step::Failed("Manual chop did not open")
        } else {
            Step::Continue
        };
    }
    if running.phase == 1 {
        if !chop::write_markers(
            base,
            running.sample,
            &running.positions[..usize::try_from(running.count).unwrap_or(0)],
            running.frames,
            running.screen,
        ) {
            return Step::Failed("native marker write failed");
        }
        running.phase = 2;
        running.deadline = now + 150;
        return Step::Continue;
    }
    if now >= running.deadline {
        let count = usize::try_from(running.count).unwrap_or(0);
        return if chop::verify(running.sample, &running.positions[..count], running.frames) {
            Step::Done(running.count)
        } else {
            Step::Failed("marker verification failed")
        };
    }
    Step::Continue
}
/// The state machine's step, for the C glue.
///
/// Returns 0 idle, 1 started, 2 continue, 3 done (with `slices`), 4 failed
/// (with `error` set to a static string).
///
/// # Safety
///
/// `slices` and `error` must be writable when non-null, and `press_chop` must be
/// a callback that replays the CHOP button.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_spectral_step(
    base: usize,
    now: i64,
    active: i32,
    screen: usize,
    press_chop: Option<unsafe extern "C" fn()>,
    slices: *mut u32,
    error: *mut *const u8,
) -> i32 {
    let step = step(base, now, active, screen, press_chop);
    match step {
        Step::Idle => 0,
        Step::Started => 1,
        Step::Continue => 2,
        Step::Done(count) => {
            if !slices.is_null() {
                // SAFETY: the caller promises this is writable.
                unsafe { *slices = count };
            }
            3
        }
        Step::Failed(message) => {
            if !error.is_null() {
                // SAFETY: the caller promises this is writable; the message is
                // a static string that outlives the call.
                unsafe { *error = message.as_ptr() };
            }
            4
        }
    }
}

/// Forget the run, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_spectral_reset() {
    reset();
}
