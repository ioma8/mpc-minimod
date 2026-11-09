//! The Spectral Chop state machine.
//!
//! Chopping the selected sample is a three-phase dance with the app: switch the
//! page to chop mode, write the boundaries through the app's own slice calls,
//! then verify what it kept. The page is a UI object that can change under us
//! between frames, so every phase re-reads the selection and gives up if it
//! moved.

use crate::backend::spectral;
use crate::chop;
use crate::page;
use crate::selection;
use mpc_sample_sdk::state::Global;

/// How many boundaries the app can take.
pub const MAX_SLICES: usize = 16;

/// What the caller should do after a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do.
    Idle,
    /// The run just started: the menu closes and the busy flag goes up.
    Started,
    /// Keep going next frame.
    Continue,
    /// Finished; the pad holds this many slices.
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

/// The run in flight.
static MACHINE: Global<Option<Machine>> = Global::new(None);

/// Run one frame of the state machine.
#[must_use]
pub fn step(now: i64) -> Step {
    match MACHINE.get_mut().as_mut() {
        None => Step::Idle,
        Some(running) => advance(now, running),
    }
}

/// Begin a run: detect the boundaries in the samples the caller read, open
/// Manual chop and store the machine.
pub fn begin(
    now: i64,
    selected: &selection::Selection,
    samples: &[f32],
    press_chop: Option<&[u8; 16]>,
) -> Step {
    if samples.len()
        != usize::try_from(selected.frames).unwrap_or(0) * usize::from(selected.channels)
    {
        return Step::Failed("cannot read sample WAV");
    }
    let Some(detected) = spectral::detect(
        samples,
        usize::from(selected.channels),
        selected.rate,
        MAX_SLICES,
    ) else {
        return Step::Failed("no onsets");
    };
    let count = detected.len().min(MAX_SLICES);
    // One boundary is still a meaningful two-region chop; only no boundary at
    // all is an error.
    if count == 0 {
        return Step::Failed("no onsets");
    }
    let mut positions = [0_u32; MAX_SLICES];
    for (index, position) in detected.iter().take(count).enumerate() {
        if *position >= selected.frames || (index > 0 && *position <= positions[index - 1]) {
            return Step::Failed("invalid onsets");
        }
        positions[index] = *position;
    }
    if !page::chop_mode(selected.screen)
        && let Some(event) = press_chop
    {
        let _ = mpc_sample_sdk::sites::replay_button(event, crate::ui::CHOP_KEY);
    }
    if !chop::set_manual(selected.screen) {
        return Step::Failed("cannot select Manual chop");
    }
    *MACHINE.get_mut() = Some(Machine {
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
fn advance(now: i64, running: &mut Machine) -> Step {
    let Some(selected) = selection::current(running.screen) else {
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
    let kind = chop::chop_kind(running.screen);
    if !page::chop_mode(running.screen) || kind != 0 {
        return if now >= running.deadline {
            Step::Failed("Manual chop did not open")
        } else {
            Step::Continue
        };
    }
    if running.phase == 1 {
        if !chop::write_markers(
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
