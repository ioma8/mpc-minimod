//! Iterative radix-2 FFT with precomputed twiddle factors.
//!
//! Both offline transforms in the mod ([`crate::hpss`], [`crate::spectral`]) need
//! a forward transform of a fixed power-of-two size and (for HPSS) the matching
//! inverse.  Allocating the twiddles once per transform instead of calling
//! `cos`/`sin` inside the butterfly loop is what makes the offline analysis
//! cheap enough to run on a worker thread while the sampler keeps playing.

use core::f32::consts::PI;
use core::fmt;
use core::ops::{Add, AddAssign, Mul, Sub};

use crate::sample::u32_to_f32;

/// A single-precision complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    /// Real part.
    pub re: f32,
    /// Imaginary part.
    pub im: f32,
}

impl Complex {
    /// The additive identity.
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };

    /// A complex number from its parts.
    #[must_use]
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    /// A real number as a complex one.
    #[must_use]
    pub const fn from_real(re: f32) -> Self {
        Self { re, im: 0.0 }
    }

    /// Magnitude, computed with `hypot` so it cannot overflow.
    #[must_use]
    pub fn magnitude(self) -> f32 {
        self.re.hypot(self.im)
    }
}

impl Add for Complex {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl AddAssign for Complex {
    fn add_assign(&mut self, rhs: Self) {
        self.re += rhs.re;
        self.im += rhs.im;
    }
}

impl Sub for Complex {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.re - rhs.re, self.im - rhs.im)
    }
}

impl Mul for Complex {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
}

/// Why an [`Fft`] could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FftError {
    /// The requested size is not a power of two.
    NotPowerOfTwo,
    /// The requested size is smaller than two points.
    TooSmall,
    /// The requested size exceeds [`Fft::MAX_SIZE`].
    TooLarge,
}

impl fmt::Display for FftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPowerOfTwo => f.write_str("FFT size must be a power of two"),
            Self::TooSmall => f.write_str("FFT size must be at least 2"),
            Self::TooLarge => f.write_str("FFT size is too large"),
        }
    }
}

impl std::error::Error for FftError {}

/// A forward/inverse transform pair of one fixed size.
#[derive(Clone, Debug)]
pub struct Fft {
    size: usize,
    /// `e^{-2*pi*i*k/size}` for `k` in `0..size/2`.
    forward: Vec<Complex>,
    /// `e^{+2*pi*i*k/size}` for `k` in `0..size/2`.
    inverse: Vec<Complex>,
    /// Bit-reversal permutation of the point index.
    reverse: Vec<u32>,
}

impl Fft {
    /// Largest supported transform size; the analysis windows are far smaller
    /// and this only guards against an accidental multi-gigabyte allocation.
    pub const MAX_SIZE: usize = 1 << 16;

    /// Build the twiddle tables for `size` points.
    ///
    /// # Errors
    ///
    /// Returns [`FftError`] when `size` is not a power of two in
    /// `2..=`[`Fft::MAX_SIZE`].
    pub fn new(size: usize) -> Result<Self, FftError> {
        if size < 2 {
            return Err(FftError::TooSmall);
        }
        if !size.is_power_of_two() {
            return Err(FftError::NotPowerOfTwo);
        }
        if size > Self::MAX_SIZE {
            return Err(FftError::TooLarge);
        }
        let half = size / 2;
        let n = u32_to_f32(size as u32);
        let mut forward = Vec::with_capacity(half);
        let mut inverse = Vec::with_capacity(half);
        for k in 0..half {
            let angle = -2.0 * PI * u32_to_f32(k as u32) / n;
            let (sin, cos) = angle.sin_cos();
            forward.push(Complex::new(cos, sin));
            inverse.push(Complex::new(cos, -sin));
        }
        let bits = size.trailing_zeros();
        let mut reverse = Vec::with_capacity(size);
        for i in 0..size {
            let index = i as u32;
            reverse.push(index.reverse_bits() >> (32 - bits));
        }
        Ok(Self {
            size,
            forward,
            inverse,
            reverse,
        })
    }

    /// Number of points per transform.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// In-place forward transform.
    ///
    /// # Panics
    ///
    /// Panics when `buffer` is not exactly [`Fft::size`] points long.  Callers
    /// in this crate size their buffers from [`Fft::size`]; the FFI boundary
    /// validates lengths before reaching here.
    pub fn forward_transform(&self, buffer: &mut [Complex]) {
        assert_eq!(buffer.len(), self.size, "FFT buffer length");
        self.permute(buffer);
        self.butterflies(buffer, &self.forward);
    }

    /// In-place inverse transform, scaled by `1/size`.
    ///
    /// # Panics
    ///
    /// Panics when `buffer` is not exactly [`Fft::size`] points long.
    pub fn inverse_transform(&self, buffer: &mut [Complex]) {
        assert_eq!(buffer.len(), self.size, "FFT buffer length");
        self.permute(buffer);
        self.butterflies(buffer, &self.inverse);
        let scale = 1.0 / u32_to_f32(self.size as u32);
        for value in buffer.iter_mut() {
            value.re *= scale;
            value.im *= scale;
        }
    }

    fn permute(&self, buffer: &mut [Complex]) {
        for (i, &target) in self.reverse.iter().enumerate() {
            let target = target as usize;
            if i < target {
                buffer.swap(i, target);
            }
        }
    }

    fn butterflies(&self, buffer: &mut [Complex], twiddles: &[Complex]) {
        let mut len = 2;
        while len <= self.size {
            let half = len / 2;
            let step = self.size / len;
            for start in (0..self.size).step_by(len) {
                for j in 0..half {
                    let w = twiddles[j * step];
                    let a = buffer[start + j];
                    let b = buffer[start + j + half] * w;
                    buffer[start + j] = a + b;
                    buffer[start + j + half] = a - b;
                }
            }
            len <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Complex, Fft, FftError};
    use crate::sample::u32_to_f32;

    #[test]
    fn rejects_bad_sizes() {
        assert!(matches!(Fft::new(0), Err(FftError::TooSmall)));
        assert!(matches!(Fft::new(3), Err(FftError::NotPowerOfTwo)));
        assert!(matches!(
            Fft::new(Fft::MAX_SIZE * 2),
            Err(FftError::TooLarge)
        ));
    }

    #[test]
    fn round_trip_restores_the_input() {
        for size in [2, 4, 16, 1024] {
            let fft = Fft::new(size).expect("size is valid");
            let mut buffer: Vec<Complex> = (0..size)
                .map(|i| {
                    let x = u32_to_f32(i as u32);
                    Complex::new(x.sin() + 0.25 * (3.0 * x).cos(), 0.0)
                })
                .collect();
            let original = buffer.clone();
            fft.forward_transform(&mut buffer);
            fft.inverse_transform(&mut buffer);
            for (got, want) in buffer.iter().zip(original.iter()) {
                assert!(
                    (got.re - want.re).abs() < 1e-5,
                    "{size}: {got:?} != {want:?}"
                );
                assert!(got.im.abs() < 1e-5, "{size}: {got:?}");
            }
        }
    }

    #[test]
    fn a_pure_tone_lands_in_one_bin() {
        let fft = Fft::new(64).expect("64 is valid");
        let mut buffer = vec![Complex::ZERO; 64];
        for (i, value) in buffer.iter_mut().enumerate() {
            let phase = 4.0 * u32_to_f32(i as u32) * core::f32::consts::TAU / 64.0;
            *value = Complex::new(phase.cos(), phase.sin());
        }
        fft.forward_transform(&mut buffer);
        // e^{i 2 pi 4 n / 64} has all of its energy in bin 4.
        let peaks: Vec<usize> = buffer
            .iter()
            .enumerate()
            .filter(|(_, c)| c.magnitude() > 30.0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(peaks, vec![4]);
        assert!((buffer[4].magnitude() - 64.0).abs() < 1e-3);
    }
}
