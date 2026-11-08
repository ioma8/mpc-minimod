//! Randomly clocked samples: the sustained excitation of the string voice.

use super::{Noise, fclamp};

/// Dust: a sparse random impulse train.
#[derive(Clone, Copy, Debug)]
pub struct Dust {
    density: f32,
}

impl Dust {
    /// A dust source at the reference's default density.
    #[must_use]
    pub fn new() -> Self {
        let mut dust = Self { density: 0.0 };
        dust.set_density(0.5);
        dust
    }

    /// Set how often an impulse appears, 0..1 (scaled to 0.3 like the
    /// reference).
    pub fn set_density(&mut self, density: f32) {
        self.density = fclamp(density, 0.0, 1.0) * 0.3;
    }

    /// The next impulse, or zero.
    pub fn process(&mut self, noise: &mut impl Noise) -> f32 {
        if self.density == 0.0 {
            return 0.0;
        }
        let inverse = 1.0 / self.density;
        let unit = noise.unit();
        if unit < self.density {
            unit * inverse
        } else {
            0.0
        }
    }
}

impl Default for Dust {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Dust;
    use crate::dsp::SequenceNoise;

    #[test]
    fn dust_is_sparse_and_bounded() {
        let mut dust = Dust::new();
        let mut noise = SequenceNoise::new(7);
        let mut hits = 0;
        for _ in 0..10_000 {
            let value = dust.process(&mut noise);
            assert!((0.0..1.0).contains(&value), "{value}");
            if value > 0.0 {
                hits += 1;
            }
        }
        // Density 0.15 means a hit roughly every seventh sample.
        assert!(hits > 500 && hits < 2_500, "{hits}");
    }

    #[test]
    fn zero_density_is_silent() {
        let mut dust = Dust::new();
        dust.set_density(0.0);
        let mut noise = SequenceNoise::new(7);
        for _ in 0..100 {
            assert!(dust.process(&mut noise).abs() < f32::EPSILON);
        }
    }
}
