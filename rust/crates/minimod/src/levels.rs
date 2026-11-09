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

/// The next shift, for the C glue: writes `out` and returns 1 when it changed.
///
/// # Safety
///
/// `out` must be writable.
pub unsafe extern "C" fn minimod_levels_shift(current: i32, delta: i32, out: *mut i32) -> i32 {
    if out.is_null() {
        return 0;
    }
    let Some(next) = shift(current, delta) else {
        return 0;
    };
    // SAFETY: the caller promises `out` is writable.
    unsafe { *out = next };
    1
}

/// The pitch-offset setter: `void (*)(void *, u64, u64, int)`.
pub type PitchOffset = unsafe extern "C" fn(usize, u64, u64, i32);

/// Call the app's pitch-offset setter with our octave shift applied.
///
/// # Safety
///
/// `original` must be the setter this hook displaced.
pub unsafe extern "C" fn minimod_pitch_off(
    original: Option<PitchOffset>,
    first: usize,
    second: u64,
    third: u64,
    applied: i32,
    shift: i32,
) {
    if let Some(setter) = original {
        // SAFETY: the caller guarantees this is the displaced setter.
        unsafe { setter(first, second, third, pitch_offset(applied, shift)) };
    }
}

/// Move the 16 Levels row by `steps` and say so.
///
/// Returns 1 when the shift was applied.
#[must_use]
pub extern "C" fn minimod_levels_bump(steps: i32) -> i32 {
    let mut next = 0;
    // SAFETY: the out pointer is this frame's.
    if unsafe { minimod_levels_shift(crate::ui::octave(), steps, core::ptr::from_mut(&mut next)) }
        == 0
    {
        return 0;
    }
    crate::ui::set_octave(next);
    let text = if next == 0 {
        "Pitch reset".to_string()
    } else {
        format!("Pitch {:+} semitones", next * 8)
    };
    crate::paint::show_toast(&text);
    1
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
