//! The oscillator and filter primitives the bass synth is built from.
//!
//! These are ports of the `DaisySP` modules the mod ships (MIT, Electrosmith /
//! Émilie Gillet).  They keep the original arithmetic order so the rendered
//! sound is the same one the C++ build produces; the only deliberate change is
//! that noise is passed in by the caller instead of coming from libc `rand()`,
//! so the synth is deterministic and testable.

use core::f32::consts::PI;

pub mod delay_line;
pub mod filters;
pub mod noise;
pub mod string;
pub mod string_voice;
pub(crate) mod window;

/// `PI` as the reference spells it: `3.1415927410125732421875f`.
pub const PI_F: f32 = PI;
/// Half of [`PI_F`].
pub const HALF_PI: f32 = PI_F * 0.5;
/// Two pi.
pub const TWO_PI: f32 = 2.0 * PI_F;
/// Semitones to octaves.
pub const ONE_TWELFTH: f32 = 1.0 / 12.0;

/// `1 / RAND_MAX` as the reference computes it: `(float)RAND_MAX` rounds up to
/// `2^31`, so the scale is exactly `2^-31`.
pub const RAND_FRAC: f32 = 1.0 / 2_147_483_648.0;

/// The reference's `fmax`: a plain comparison, not a NaN-propagating maximum.
#[must_use]
pub fn fmax(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// The reference's `fmin`: a plain comparison.
#[must_use]
pub fn fmin(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

/// `fmin(fmax(in, min), max)`.
#[must_use]
pub fn fclamp(value: f32, min: f32, max: f32) -> f32 {
    fmin(fmax(value, min), max)
}

/// One-pole smoothing, in place.
pub fn fonepole(out: &mut f32, input: f32, coefficient: f32) {
    *out += coefficient * (input - *out);
}

/// A source of white noise already scaled into `[0, 1]`, which is what
/// `rand() * kRandFrac` produces in the reference.
pub trait Noise {
    /// The next value, in `[0, 1]`.
    fn unit(&mut self) -> f32;
}

/// A small xorshift generator, the default noise source on the device.
///
/// It replaces libc `rand()`, whose sequence is process-global and differs
/// between the host and the device; a per-voice generator makes the rendered
/// sound reproducible and keeps two threads from sharing hidden state.
#[derive(Clone, Copy, Debug)]
pub struct WhiteNoise {
    state: u32,
}

impl WhiteNoise {
    /// A generator with a fixed seed.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 0x9e37_79b9 } else { seed },
        }
    }
}

impl Default for WhiteNoise {
    fn default() -> Self {
        Self::new(0x1234_5678)
    }
}

impl Noise for WhiteNoise {
    fn unit(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        // 31 bits, like RAND_MAX.
        let raw = self.state >> 1;
        (raw as f32) * RAND_FRAC
    }
}

/// The exact sequence the golden fixtures were recorded with: the same 31-bit
/// linear congruential generator the reference build is compiled against.
#[derive(Clone, Copy, Debug)]
pub struct SequenceNoise {
    state: u32,
}

impl SequenceNoise {
    /// The generator the fixtures start from.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self { state: seed }
    }
}

impl Noise for SequenceNoise {
    fn unit(&mut self) -> f32 {
        self.state = self.state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let raw = self.state & 0x7fff_ffff;
        (raw as f32) * RAND_FRAC
    }
}

#[cfg(test)]
mod tests {
    use super::{Noise, SequenceNoise, WhiteNoise, fclamp, fmax, fmin, fonepole};

    #[test]
    fn clamps_like_the_reference() {
        assert!((fclamp(0.5, 0.0, 1.0) - 0.5).abs() < f32::EPSILON);
        assert!(fclamp(-1.0, 0.0, 1.0).abs() < f32::EPSILON);
        assert!((fclamp(2.0, 0.0, 1.0) - 1.0).abs() < f32::EPSILON);
        // A plain comparison, so a NaN first argument falls through to the
        // second one instead of propagating.
        assert!((fmax(f32::NAN, 1.0) - 1.0).abs() < f32::EPSILON);
        assert!((fmin(f32::NAN, 1.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn one_pole_moves_towards_the_input() {
        let mut state = 0.0;
        for _ in 0..100 {
            fonepole(&mut state, 1.0, 0.1);
        }
        assert!(state > 0.99 && state < 1.0, "{state}");
    }

    #[test]
    fn sequence_noise_is_stable_and_stays_in_range() {
        let mut noise = SequenceNoise::new(1);
        let mut first = Vec::new();
        for _ in 0..8 {
            let value = noise.unit();
            assert!((0.0..=1.0).contains(&value), "{value}");
            first.push(value);
        }
        let mut again = SequenceNoise::new(1);
        for expected in first {
            assert_eq!(again.unit().to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn white_noise_stays_in_range() {
        let mut noise = WhiteNoise::default();
        for _ in 0..1000 {
            let value = noise.unit();
            assert!((0.0..=1.0).contains(&value), "{value}");
        }
    }
}
