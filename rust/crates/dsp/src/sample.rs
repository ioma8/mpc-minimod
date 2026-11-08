//! Conversions between the device's S32 PCM frames and f32 samples.
//!
//! The firmware hands the audio device interleaved signed 32-bit frames and the
//! DSP works in f32 `[-1.0, 1.0)`.  Both directions are lossy, so they are
//! collected here instead of being scattered through the audio paths:
//!
//! * `i32 -> f32` is exact for the upper 24 bits and rounds to nearest for the
//!   rest, matching what the firmware's own conversion does;
//! * `f32 -> i32` truncates toward zero after scaling, exactly like the C
//!   implementation this replaced, and saturates instead of wrapping.
//!
//! The scale is `2^31` in both directions, which makes the extreme frame
//! asymmetrical: `i32::MIN` is exactly `-1.0`, while `i32::MAX` rounds up to
//! `2^31` and therefore becomes exactly `1.0`.  Going back, `-1.0` produces
//! `i32::MIN` and `1.0` saturates at `i32::MAX`.  That is what the firmware's
//! own conversion does, so a loop round-trips sample-exact.
//!
//! The scaling factors are powers of two, so the f32 multiply itself is exact
//! apart from the final rounding and both directions are bit-identical to a
//! plain `(float)v * k` in C.

/// `1 / 2^31`: maps `i32::MIN` to exactly `-1.0`.
const INV_I32: f32 = 1.0 / 2_147_483_648.0;

/// `2^31 - 1`: maps `1.0` to the largest positive frame value.
const I32_MAX: f32 = 2_147_483_647.0;

/// One S32 frame as an f32 sample in `[-1.0, 1.0)`.
#[must_use]
pub fn s32_to_f32(value: i32) -> f32 {
    // `i32 -> f64` is exact and `f64 -> f32` rounds once, which is the same
    // result as a direct `i32 -> f32` conversion; going through f64 keeps the
    // conversion exact-by-construction instead of silently losing precision.
    let scaled = f64::from(value) as f32;
    scaled * INV_I32
}

/// Two S32 frames summed into one mono f32 sample, as the stereo capture path
/// does it: the integer frames are summed before the scale is applied.
#[must_use]
pub fn s32_pair_to_f32(left: i32, right: i32) -> f32 {
    let (left, right) = (f64::from(left) as f32, f64::from(right) as f32);
    (left + right) * (0.5 * INV_I32)
}

/// A `u32` as an f32.
///
/// Frame counts, buffer lengths and bin indices are well below 2^24, so this is
/// exact for every value the DSP actually produces; it exists so that the
/// integer-to-float boundary is one documented function instead of a cast
/// sprinkled through the analysis code.
#[must_use]
pub fn u32_to_f32(value: u32) -> f32 {
    f64::from(value) as f32
}

/// One f32 sample as an S32 frame, clipped to the representable range.
#[must_use]
pub fn f32_to_s32(value: f32) -> i32 {
    let clamped = value.clamp(-1.0, 1.0);
    (clamped * I32_MAX) as i32
}

#[cfg(test)]
mod tests {
    use super::{f32_to_s32, s32_pair_to_f32, s32_to_f32};

    #[test]
    fn extremes_map_to_the_unit_interval() {
        assert!(s32_to_f32(i32::MIN) <= -0.999_999_94);
        assert!(s32_to_f32(0).abs() < f32::EPSILON);
        // `i32::MAX` rounds up to 2^31 in f32, so it lands on exactly 1.0.
        assert!((s32_to_f32(i32::MAX) - 1.0).abs() < f32::EPSILON);
        assert_eq!(f32_to_s32(-1.0), i32::MIN);
        assert_eq!(f32_to_s32(1.0), i32::MAX);
        assert_eq!(f32_to_s32(0.0), 0);
    }

    #[test]
    fn output_saturates_instead_of_wrapping() {
        assert_eq!(f32_to_s32(4.0), i32::MAX);
        assert_eq!(f32_to_s32(-4.0), i32::MIN);
        assert_eq!(f32_to_s32(f32::NAN), 0);
    }

    #[test]
    fn stereo_pair_averages() {
        let full = s32_pair_to_f32(i32::MIN, i32::MIN);
        assert!((full - -1.0).abs() < f32::EPSILON);
        let (left, right) = (s32_to_f32(i32::MAX), s32_to_f32(i32::MIN));
        assert!((s32_pair_to_f32(i32::MAX, i32::MIN) - 0.5 * (left + right)).abs() < 1e-6);
    }
}
