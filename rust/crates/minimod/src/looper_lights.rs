//! Per-pad lighting for the Live Looper.
//!
//! The looper's state is published in its snapshot; this module maps it to the
//! SDK's per-pad AC50 overrides. The app's native colours are restored when the
//! fullscreen view closes.

use core::sync::atomic::{AtomicU8, Ordering};

use crate::backend::looper::{PADS, State, View};
use mpc_sample_sdk::platform::lighting;

/// An unused pad: dim neutral, visible but clearly not a loop.
const EMPTY: (u32, f32) = (0x00ff_ffff, 0.06);
/// Recording a new take.
const REC: (u32, f32) = (0x00ff_0000, 1.0);
/// Overdubbing onto a playing loop.
const DUB: (u32, f32) = (0x00ff_b000, 1.0);
/// A playing loop.
const PLAY: (u32, f32) = (0x0000_ff00, 1.0);
/// A loop that is kept but paused.
const STOP: (u32, f32) = (0x0000_ff00, 0.22);

/// Sentinel in the state cache: no light has been written for this pad.
const UNSET: u8 = u8::MAX;

/// The last state code written for every pad.
static SEEN: [AtomicU8; PADS] = [const { AtomicU8::new(UNSET) }; PADS];

/// The colour and brightness for one pad state.
const fn style(state: State) -> (u32, f32) {
    match state {
        State::Rec | State::Drain => REC,
        State::Dub => DUB,
        State::Play => PLAY,
        State::Stop => STOP,
        State::Idle => EMPTY,
    }
}

/// The cache code for one pad state.
const fn code(state: State) -> u8 {
    match state {
        State::Idle => 0,
        State::Rec | State::Drain => 1,
        State::Play => 2,
        State::Dub => 3,
        State::Stop => 4,
    }
}

/// Start the looper lighting: every pad empty until the first snapshot.
pub fn show() {
    for (index, seen) in SEEN.iter().enumerate() {
        seen.store(code(State::Idle), Ordering::Release);
        let Ok(pad) = u8::try_from(index) else {
            continue;
        };
        let _ = lighting::send_pad_light(pad, EMPTY.0, EMPTY.1);
    }
}

/// Update the pads whose state changed in this snapshot.
pub fn sync(view: &View) {
    for (index, pad) in view.pads.iter().enumerate() {
        let state = State::from_raw(i32::from(pad.state));
        let next = code(state);
        if SEEN[index].swap(next, Ordering::AcqRel) == next {
            continue;
        }
        let Ok(index) = u8::try_from(index) else {
            continue;
        };
        let (rgb, brightness) = style(state);
        let _ = lighting::send_pad_light(index, rgb, brightness);
    }
}

/// Stop the looper lighting and restore the app's native pad colours.
pub fn hide() {
    lighting::restore_pad_rgbs();
    for seen in &SEEN {
        seen.store(UNSET, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{EMPTY, REC, STOP, style};
    use crate::backend::looper::State;

    #[test]
    fn every_looper_state_has_a_light() {
        assert_eq!(style(State::Idle), EMPTY);
        assert_eq!(style(State::Rec), REC);
        assert_eq!(style(State::Drain), REC);
        assert_eq!(style(State::Play), (0x0000_ff00, 1.0));
        assert_eq!(style(State::Dub), (0x00ff_b000, 1.0));
        assert_eq!(style(State::Stop), STOP);
        assert!(STOP.1 < 0.5 && STOP.1 > 0.0);
    }
}
