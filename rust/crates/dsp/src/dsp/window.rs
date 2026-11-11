//! Analysis windows shared by the offline transforms.

use core::f32::consts::TAU;

use crate::sample::u32_to_f32;

/// A symmetric Hann window of `length` points.
///
/// HPSS and Spectral Chop use the same window; keeping one definition means a
/// change to it cannot leave the two analyses with different sides.
#[must_use]
pub(crate) fn hann(length: usize) -> Vec<f32> {
    if length <= 1 {
        return vec![1.0; length];
    }
    let denominator = u32_to_f32(u32::try_from(length - 1).unwrap_or(1));
    (0..length)
        .map(|index| {
            let phase = TAU * u32_to_f32(u32::try_from(index).unwrap_or(0)) / denominator;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}
