//! A fixed-capacity delay line with linear, Hermite and all-pass reads.
//!
//! The capacity is a const parameter because the reference sizes its lines on
//! the stack (a 1024-sample string and a 256-sample stretch line); keeping that
//! shape means the audio path never allocates after construction.

/// A circular delay line of `N` samples.
#[derive(Clone)]
pub struct DelayLine<const N: usize> {
    line: [f32; N],
    write: usize,
}

impl<const N: usize> DelayLine<N> {
    /// A cleared line.
    #[must_use]
    pub fn new() -> Self {
        Self {
            line: [0.0; N],
            write: 0,
        }
    }

    /// Clear the line and rewind the write head.
    pub fn reset(&mut self) {
        self.line.fill(0.0);
        self.write = 0;
    }

    /// Push one sample; the write head then moves one step back, so a delay of
    /// one reads what was written last.
    pub fn write(&mut self, sample: f32) {
        self.line[self.write] = sample;
        self.write = (self.write + N - 1) % N;
    }

    /// Read at a fractional delay with linear interpolation.
    #[must_use]
    pub fn read(&self, delay: f32) -> f32 {
        let integral = integral_delay(delay);
        let fraction = delay - integral as f32;
        let a = self.line[(self.write + integral) % N];
        let b = self.line[(self.write + integral + 1) % N];
        a + (b - a) * fraction
    }

    /// Read at a fractional delay with a 4-point Hermite curve.
    #[must_use]
    pub fn read_hermite(&self, delay: f32) -> f32 {
        let integral = integral_delay(delay);
        let fraction = delay - integral as f32;
        let base = self.write + integral + N;
        let xm1 = self.line[(base - 1) % N];
        let x0 = self.line[base % N];
        let x1 = self.line[(base + 1) % N];
        let x2 = self.line[(base + 2) % N];
        let c = (x1 - xm1) * 0.5;
        let v = x0 - x1;
        let w = c + v;
        let a = w + v + (x2 - x0) * 0.5;
        let b_neg = w + a;
        (((a * fraction) - b_neg) * fraction + c) * fraction + x0
    }

    /// One all-pass step, used for the string's dispersion.
    pub fn allpass(&mut self, sample: f32, delay: usize, coefficient: f32) -> f32 {
        let read = self.line[(self.write + delay) % N];
        let write = sample + coefficient * read;
        self.write(write);
        -write * coefficient + read
    }
}

impl<const N: usize> Default for DelayLine<N> {
    fn default() -> Self {
        Self::new()
    }
}

use crate::num::f32_to_usize;

/// The integer part of a fractional delay, truncated towards zero like the
/// reference's `static_cast<int32_t>` then used as an unsigned offset.
fn integral_delay(delay: f32) -> usize {
    if delay <= 0.0 {
        0
    } else {
        // A negative or absurd delay cannot reach here: callers clamp it to the
        // line's range first.
        f32_to_usize(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::DelayLine;

    #[test]
    fn a_delay_of_one_returns_the_last_sample() {
        let mut line = DelayLine::<8>::new();
        line.write(0.25);
        assert_eq!(line.read(1.0).to_bits(), 0.25_f32.to_bits());
        line.write(0.5);
        assert_eq!(line.read(1.0).to_bits(), 0.5_f32.to_bits());
        assert_eq!(line.read(2.0).to_bits(), 0.25_f32.to_bits());
    }

    #[test]
    fn interpolation_is_linear_between_taps() {
        let mut line = DelayLine::<8>::new();
        line.write(0.0);
        line.write(1.0);
        // Delay 1 is the newest sample, delay 2 the one before it.
        assert_eq!(line.read(1.0).to_bits(), 1.0_f32.to_bits());
        assert!((line.read(1.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hermite_matches_the_taps_at_integer_delays() {
        let mut line = DelayLine::<8>::new();
        for value in [0.1_f32, 0.2, 0.3, 0.4] {
            line.write(value);
        }
        assert!((line.read_hermite(1.0) - 0.4).abs() < 1e-6);
        assert!((line.read_hermite(2.0) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn allpass_is_lossless_for_a_zero_coefficient() {
        let mut line = DelayLine::<8>::new();
        let out = line.allpass(0.75, 1, 0.0);
        assert!(out.abs() < f32::EPSILON);
        assert_eq!(line.read(1.0).to_bits(), 0.75_f32.to_bits());
    }
}
