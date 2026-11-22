//! 16 Levels: the octave shift the PLUS/MINUS buttons apply, and the microtonal
//! half-step grid the DATA encoder toggles.
//!
//! In 16 Levels the pad grid is a chromatic keyboard; PLUS/MINUS shift the whole
//! row by an octave, and the shift is applied through the app's own pitch-offset
//! setter so the pad's stored tuning is never touched.
//!
//! That setter carries whole semitones only: its property slot quantizes the
//! pitch parameter to one step per semitone.  The app's parameter has a fine
//! tune next to the semitones, in the slot after it, and one step of that slot
//! is a tenth of a semitone, so the microtonal grid is expressed as semitones
//! plus 0 or 5 fine steps — never as a fraction of a semitone, which the
//! semitone slot cannot carry.

/// Semitones one press moves the row by.
pub const SEMITONES_PER_STEP: i32 = 8;
/// How far the shift may go, in presses.
pub const STEP_LIMIT: i32 = 4;
/// Fine-tune steps in half a semitone.  The firmware's fine-tune slot steps in
/// tenths of a semitone (measured on 1.3.0.12), so a half step is five steps.
pub const FINE_STEPS_PER_HALF_STEP: i32 = 5;

/// The pitch of one 16 Levels pad, split the way the app's parameter set is:
/// whole semitones plus a fine tune in tenths of a semitone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pitch {
    /// Whole semitones, from the app's semitone slot.
    pub semitones: i32,
    /// Fine-tune steps added to `semitones`; `0` or [`FINE_STEPS_PER_HALF_STEP`].
    pub fine: i32,
}

/// The pitch for one pad.
///
/// `applied` is the offset the app computed — one semitone per level step —
/// `shift` is the PLUS/MINUS shift in steps, and `microtonal` halves the level
/// grid so each pad is half a semitone above the one before it.  The shift keeps
/// its eight semitones in both grids: one full microtonal row, so a press lands
/// on the next row rather than in the middle of this one.
#[must_use]
pub fn pitch(applied: i32, shift: i32, microtonal: bool) -> Pitch {
    // Half steps, so the microtonal grid stays exact integer arithmetic.  The
    // shift is always counted in half steps: eight semitones, both grids.
    let grid = if microtonal { applied } else { 2 * applied };
    let half = grid + 2 * SEMITONES_PER_STEP * shift;
    Pitch {
        // `div_euclid`/`rem_euclid` keep the fine tune positive for the rows
        // below the pad's own pitch: -1 half step is -1 semitone and +5 steps.
        semitones: half.div_euclid(2),
        fine: if microtonal {
            half.rem_euclid(2) * FINE_STEPS_PER_HALF_STEP
        } else {
            0
        },
    }
}

/// The next shift, or `None` when it is already at the limit in that direction.
#[must_use]
pub fn shift(current: i32, delta: i32) -> Option<i32> {
    let next = (current + delta).clamp(-STEP_LIMIT, STEP_LIMIT);
    if next == current { None } else { Some(next) }
}

/// The semitone offset to hand the app for an offset it computed and our shift.
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

/// Number of times the firmware pitch hook has run (device test probe).
#[unsafe(no_mangle)]
pub static g_levels_pitch_calls: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Most recent value the firmware pitch hook returned (device test probe).
#[unsafe(no_mangle)]
pub static g_levels_pitch_result: core::sync::atomic::AtomicI32 =
    core::sync::atomic::AtomicI32::new(0);
/// Most recent fine-tune steps the firmware's fine slot received (device probe).
#[unsafe(no_mangle)]
pub static g_levels_fine_result: core::sync::atomic::AtomicI32 =
    core::sync::atomic::AtomicI32::new(0);

/// Apply the 16 Levels grid and shift to an offset the app computed.
#[must_use]
pub fn apply(applied: i32) -> i32 {
    let pitch = pitch(applied, crate::ui::octave(), crate::ui::microtonal());
    g_levels_pitch_calls.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    g_levels_pitch_result.store(pitch.semitones, core::sync::atomic::Ordering::Relaxed);
    g_levels_fine_result.store(pitch.fine, core::sync::atomic::Ordering::Relaxed);
    pitch.semitones
}

/// The fine-tune steps for the same pad, written in the fine-tune slot.
#[must_use]
pub fn apply_fine(applied: i32) -> i32 {
    pitch(applied, crate::ui::octave(), crate::ui::microtonal()).fine
}

#[cfg(test)]
mod tests {
    use super::{
        FINE_STEPS_PER_HALF_STEP, Pitch, SEMITONES_PER_STEP, STEP_LIMIT, pitch, pitch_offset, shift,
    };

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

    #[test]
    fn whole_semitones_are_unchanged_without_the_microtonal_grid() {
        assert_eq!(
            pitch(0, 0, false),
            Pitch {
                semitones: 0,
                fine: 0
            }
        );
        assert_eq!(
            pitch(3, 0, false),
            Pitch {
                semitones: 3,
                fine: 0
            }
        );
        assert_eq!(
            pitch(3, 1, false),
            Pitch {
                semitones: 11,
                fine: 0
            }
        );
        assert_eq!(
            pitch(3, -2, false),
            Pitch {
                semitones: -13,
                fine: 0
            }
        );
    }

    #[test]
    fn the_microtonal_grid_puts_a_pad_half_a_semitone_apart() {
        assert_eq!(
            pitch(0, 0, true),
            Pitch {
                semitones: 0,
                fine: 0
            }
        );
        assert_eq!(
            pitch(1, 0, true),
            Pitch {
                semitones: 0,
                fine: 5
            }
        );
        assert_eq!(
            pitch(2, 0, true),
            Pitch {
                semitones: 1,
                fine: 0
            }
        );
        assert_eq!(
            pitch(3, 0, true),
            Pitch {
                semitones: 1,
                fine: 5
            }
        );
        assert_eq!(
            pitch(15, 0, true),
            Pitch {
                semitones: 7,
                fine: 5
            },
            "the row spans 8 semitones"
        );
    }

    #[test]
    fn the_microtonal_grid_keeps_the_fine_tune_positive_below_the_pad() {
        assert_eq!(
            pitch(-1, 0, true),
            Pitch {
                semitones: -1,
                fine: 5
            }
        );
        assert_eq!(
            pitch(-2, 0, true),
            Pitch {
                semitones: -1,
                fine: 0
            }
        );
        assert_eq!(
            pitch(-3, 0, true),
            Pitch {
                semitones: -2,
                fine: 5
            }
        );
    }

    #[test]
    fn the_shift_stays_eight_semitones_in_both_grids() {
        assert_eq!(
            pitch(0, 1, true),
            Pitch {
                semitones: 8,
                fine: 0
            }
        );
        assert_eq!(
            pitch(1, 1, true),
            Pitch {
                semitones: 8,
                fine: 5
            }
        );
        assert_eq!(
            pitch(1, -1, true),
            Pitch {
                semitones: -8,
                fine: 5
            }
        );
        assert_eq!(FINE_STEPS_PER_HALF_STEP, 5);
    }
}
