//! The Spectral Chop state machine.
//!
//! Chopping the selected sample is a three-phase dance with the app: switch the
//! page to chop mode, write the boundaries through the app's own slice calls,
//! then verify what it kept. The page is a UI object that can change under us
//! between frames, so every phase re-reads the selection and gives up if it
//! moved.

use crate::chop;
use crate::page;
use crate::selection;
use mpc_sample_sdk::state::Global;

/// How many boundaries the app can take.
pub const MAX_SLICES: usize = 16;
/// How long the marker phases may take.
const MARKER_DEADLINE_MS: i64 = 3_000;
/// How long the app may take to show Manual chop before verification.
const VERIFY_DELAY_MS: i64 = 150;

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
    // SAFETY: `MACHINE` is owned by the app's UI thread.
    unsafe {
        MACHINE.with_mut(|machine| {
            let Some(running) = machine.as_mut() else {
                return Step::Idle;
            };
            let step = advance(now, running);
            if matches!(step, Step::Done(_) | Step::Failed(_)) {
                *machine = None;
            }
            step
        })
    }
}

/// Begin a run: validate the boundaries the worker detected, open Manual chop
/// and store the machine.
#[must_use]
pub fn begin(
    now: i64,
    selected: &selection::Selection,
    detected: &[u32],
    press_chop: Option<(&[u8; 16], usize)>,
) -> Step {
    let count = detected.len().min(MAX_SLICES);
    // One boundary is still a meaningful two-region chop; only no boundary at
    // all is an error.
    if count == 0 {
        return Step::Failed("no onsets");
    }
    let mut positions = [0_u32; MAX_SLICES];
    for (index, position) in detected.iter().take(count).enumerate() {
        if *position >= selected.frames() || (index > 0 && *position <= positions[index - 1]) {
            return Step::Failed("invalid onsets");
        }
        positions[index] = *position;
    }
    // SAFETY: the selection is live and came from `selection::current`.
    let manual = unsafe { page::chop_mode(selected.screen()) };
    // SAFETY: the same live page the selection came from.
    let visible = unsafe { page::visible(selected.screen()) };
    if !manual
        && visible
        && let Some((event, screen)) = press_chop
        && screen == selected.screen()
    {
        // The event was captured on this page and the page is still visible, so
        // its context pointer is still the one the app handed the button hook.
        // SAFETY: as above.
        let _ = unsafe { mpc_sample_sdk::sites::replay_button(event, crate::ui::CHOP_KEY) };
    }
    // SAFETY: the page is the live Sample page behind the selection.
    if !unsafe { chop::set_manual(selected.screen()) } {
        return Step::Failed("cannot select Manual chop");
    }
    // SAFETY: `MACHINE` is owned by the app's UI thread and nothing else holds
    // a borrow of it here.
    unsafe {
        MACHINE.with_mut(|slot| {
            *slot = Some(Machine {
                screen: selected.screen(),
                state: selected.state(),
                sample: selected.sample(),
                frames: selected.frames(),
                channels: selected.channels(),
                rate: selected.rate(),
                count: u32::try_from(count).unwrap_or(0),
                positions,
                phase: 1,
                deadline: now + MARKER_DEADLINE_MS,
            });
        });
    }
    Step::Started
}

/// Phases 1 and 2: wait for Manual chop, write the boundaries, verify them.
fn advance(now: i64, running: &mut Machine) -> Step {
    // SAFETY: `running.screen` is the page the chop started on; this callback
    // still runs on its UI thread.
    let Some(selected) = (unsafe { selection::current(running.screen) }) else {
        return Step::Failed("sample changed; cancelled");
    };
    if selected.screen() != running.screen
        || selected.state() != running.state
        || selected.sample() != running.sample
        || selected.frames() != running.frames
        || selected.channels() != running.channels
        || selected.rate() != running.rate
    {
        return Step::Failed("sample changed; cancelled");
    }
    let kind = chop::chop_kind(running.screen);
    // SAFETY: the page is the live Sample page this chop started on.
    if !unsafe { page::chop_mode(running.screen) } || kind != 0 {
        return if now >= running.deadline {
            Step::Failed("Manual chop did not open")
        } else {
            Step::Continue
        };
    }
    if running.phase == 1 {
        // SAFETY: the sample and page are the live objects captured when the
        // chop started, on this UI thread.
        if !unsafe {
            chop::write_markers(
                running.sample,
                &running.positions[..usize::try_from(running.count).unwrap_or(0)],
                running.frames,
                running.screen,
            )
        } {
            return Step::Failed("native marker write failed");
        }
        running.phase = 2;
        running.deadline = now + VERIFY_DELAY_MS;
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

#[cfg(test)]
mod tests {
    use super::{Step, begin};
    use crate::selection::Selection;

    fn selection() -> Selection {
        // SAFETY: the value is only read for its geometry.
        unsafe { Selection::from_parts(1, 2, 3, 1_000, 2, 44_100) }
    }

    #[test]
    fn an_empty_detection_is_refused() {
        assert_eq!(begin(0, &selection(), &[], None), Step::Failed("no onsets"));
    }

    #[test]
    fn a_boundary_at_or_past_the_end_is_refused() {
        assert_eq!(
            begin(0, &selection(), &[1_000], None),
            Step::Failed("invalid onsets")
        );
        assert_eq!(
            begin(0, &selection(), &[10, 10], None),
            Step::Failed("invalid onsets")
        );
    }
}
