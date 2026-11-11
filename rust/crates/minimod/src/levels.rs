//! 16 Levels: the octave shift the PLUS/MINUS buttons apply.
//!
//! In 16 Levels the pad grid is a chromatic keyboard; PLUS/MINUS shift the whole
//! row by an octave, and the shift is applied through the app's own pitch-offset
//! setter so the pad's stored tuning is never touched.

/// Semitones one press moves the row by.
pub const SEMITONES_PER_STEP: i32 = 8;
/// How far the shift may go, in presses.
pub const STEP_LIMIT: i32 = 4;

/// The next shift, or `None` when it is already at the limit in that direction.
#[must_use]
pub fn shift(current: i32, delta: i32) -> Option<i32> {
    let next = (current + delta).clamp(-STEP_LIMIT, STEP_LIMIT);
    if next == current { None } else { Some(next) }
}

/// The pitch offset to hand the app for an offset it computed and our shift.
#[must_use]
pub fn pitch_offset(applied: i32, shift: i32) -> i32 {
    applied + SEMITONES_PER_STEP * shift
}

/// The next 16 Levels row and the toast it should show.
#[must_use]
pub fn bump(current: i32, steps: i32) -> Option<(i32, String)> {
    let next = shift(current, steps)?;
    let text = if next == 0 {
        "Pitch reset".to_string()
    } else {
        format!("Pitch {:+} semitones", next * SEMITONES_PER_STEP)
    };
    Some((next, text))
}

/// Apply the 16 Levels octave shift to an offset the app computed.
#[must_use]
pub fn apply(applied: i32) -> i32 {
    pitch_offset(applied, crate::ui::octave())
}

#[cfg(test)]
mod tests {
    use super::{SEMITONES_PER_STEP, STEP_LIMIT, pitch_offset, shift};

    #[test]
    fn the_shift_clamps_at_both_ends() {
        assert_eq!(shift(0, 1), Some(1));
        assert_eq!(shift(3, 1), Some(4));
        assert_eq!(shift(4, 1), None, "already at the top");
        assert_eq!(shift(-4, -1), None, "already at the bottom");
        assert_eq!(shift(0, 1).map(|s| pitch_offset(0, s)), Some(8));
    }

    #[test]
    fn the_pitch_offset_adds_eight_semitones_per_step() {
        assert_eq!(pitch_offset(3, 0), 3, "no shift is a pass-through");
        assert_eq!(pitch_offset(3, 1), 3 + SEMITONES_PER_STEP);
        assert_eq!(pitch_offset(3, -2), 3 - 2 * SEMITONES_PER_STEP);
        assert_eq!(SEMITONES_PER_STEP, 8);
        assert_eq!(STEP_LIMIT, 4);
    }
}
