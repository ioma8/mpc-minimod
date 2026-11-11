//! Explicit, saturating conversions for the analysis code.
//!
//! Thresholds and window lengths are computed in f32 and then used as sample or
//! bin indices.  These helpers make that step explicit and total: no panic, no
//! wrap-around, and a NaN simply lands on zero.

/// A non-negative f32 as a `u32`, saturating at both ends.
///
/// Negative values (and NaN) become zero; values beyond `u32::MAX` saturate.
#[must_use]
pub fn f32_to_u32(value: f32) -> u32 {
    // `as i64` on a float saturates in Rust, so this cannot overflow; the
    // explicit `max` only decides what a negative or NaN input means.
    u32::try_from(value.max(0.0) as i64).unwrap_or(u32::MAX)
}

/// A non-negative f32 as a `usize`, saturating.
#[must_use]
pub fn f32_to_usize(value: f32) -> usize {
    f32_to_u32(value) as usize
}

/// A `u32` as a `usize`; always exact because `usize` is at least 32 bits.
#[must_use]
pub fn u32_to_usize(value: u32) -> usize {
    value as usize
}

/// A `usize` as an `i64`, saturating.
#[must_use]
pub fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// An `i64` as a `usize`, saturating at zero.
#[must_use]
pub fn i64_to_usize(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}
