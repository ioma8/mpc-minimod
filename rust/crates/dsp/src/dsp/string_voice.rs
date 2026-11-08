//! The extended Karplus-Strong voice: an excitation filter, an optional sustain
//! noise, and the string itself.
//!
//! Port of the reference's `StringVoice`, keeping its arithmetic order.  The
//! excitation noise and the sustain dust take the caller's [`Noise`] instead of
//! libc `rand()`.

use super::filters::Svf;
use super::noise::Dust;
use super::string::String;
use super::{Noise, ONE_TWELFTH, fclamp, fmin};
use crate::num::f32_to_usize;

/// A plucked-string voice.
#[derive(Clone)]
pub struct StringVoice {
    sample_rate: f32,
    string: String,
    excitation_filter: Svf,
    dust: Dust,
    f0: f32,
    accent: f32,
    brightness: f32,
    damping: f32,
    density: f32,
    sustain: bool,
    trig: bool,
    remaining_noise: usize,
    aux: f32,
}

impl StringVoice {
    /// A voice at the reference's default settings.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut voice = Self {
            sample_rate,
            string: String::new(sample_rate),
            excitation_filter: Svf::new(sample_rate),
            dust: Dust::new(),
            f0: 0.0,
            accent: 0.8,
            brightness: 0.2,
            damping: 0.7,
            density: 0.0,
            sustain: false,
            trig: false,
            remaining_noise: 0,
            aux: 0.0,
        };
        voice.set_sustain(false);
        voice.set_frequency(440.0);
        voice.set_accent(0.8);
        voice.set_structure(0.7);
        voice.set_brightness(0.2);
        voice.set_damping(0.7);
        voice
    }

    /// Clear the string.
    pub fn reset(&mut self) {
        self.string.reset();
    }

    /// Keep exciting the string with noise instead of plucking it.
    pub const fn set_sustain(&mut self, sustain: bool) {
        self.sustain = sustain;
    }

    /// Strike the string on the next sample.
    pub const fn trigger(&mut self) {
        self.trig = true;
    }

    /// Set the root frequency in Hz.
    pub fn set_frequency(&mut self, frequency: f32) {
        self.string.set_frequency(frequency);
        self.f0 = fclamp(frequency / self.sample_rate, 0.0, 0.25);
    }

    /// Hit the string harder, 0..1.
    pub fn set_accent(&mut self, accent: f32) {
        self.accent = fclamp(accent, 0.0, 1.0);
    }

    /// Set the string type: below 0.24 a curved bridge, above 0.26 dispersion.
    pub fn set_structure(&mut self, structure: f32) {
        let structure = fclamp(structure, 0.0, 1.0);
        let non_linearity = if structure < 0.24 {
            (structure - 0.24) * 4.166
        } else if structure > 0.26 {
            (structure - 0.26) * 1.351_35
        } else {
            0.0
        };
        self.string.set_non_linearity(non_linearity);
    }

    /// Set the brightness, 0..1.
    pub fn set_brightness(&mut self, brightness: f32) {
        self.brightness = fclamp(brightness, 0.0, 1.0);
        self.density = self.brightness * self.brightness;
    }

    /// Set the decay time relative to the accent, 0..1.
    pub fn set_damping(&mut self, damping: f32) {
        self.damping = fclamp(damping, 0.0, 1.0);
    }

    /// The excitation signal that fed the string, for metering.
    #[must_use]
    pub const fn aux(&self) -> f32 {
        self.aux
    }

    /// Render the next sample.
    pub fn process(&mut self, trigger: bool, noise: &mut impl Noise) -> f32 {
        let brightness = self.brightness + 0.25 * self.accent * (1.0 - self.brightness);
        let damping = self.damping + 0.25 * self.accent * (1.0 - self.damping);

        if trigger || self.trig || self.sustain {
            self.trig = false;
            let range = 72.0;
            let f = 4.0 * self.f0;
            let cutoff = fmin(
                f * 2.0_f32.powf(ONE_TWELFTH * (brightness * (2.0 - brightness) - 0.5) * range),
                0.499,
            );
            let q = if self.sustain { 1.0 } else { 0.5 };
            self.remaining_noise = if self.f0 > 0.0 {
                f32_to_usize(1.0 / self.f0)
            } else {
                0
            };
            self.excitation_filter
                .set_frequency(cutoff * self.sample_rate);
            self.excitation_filter.set_resonance(q);
        }

        let mut excitation = 0.0;
        if self.sustain {
            let dust_f = 0.000_05 + 0.999_95 * self.density * self.density;
            self.dust.set_density(dust_f);
            excitation = self.dust.process(noise) * (8.0 - dust_f * 6.0) * self.accent;
        } else if self.remaining_noise != 0 {
            excitation = 2.0 * noise.unit() - 1.0;
            self.remaining_noise -= 1;
        }

        self.excitation_filter.process(excitation);
        let temp = self.excitation_filter.low_pass();
        self.aux = temp;

        self.string.set_brightness(brightness);
        self.string.set_damping(damping);
        self.string.process(temp, noise)
    }
}

#[cfg(test)]
mod tests {
    use super::StringVoice;
    use crate::dsp::WhiteNoise;

    #[test]
    fn a_triggered_voice_sounds_then_falls_silent() {
        let rate = 44_100.0_f32;
        let mut voice = StringVoice::new(rate);
        voice.set_frequency(110.0);
        voice.set_accent(0.8);
        voice.set_structure(0.7);
        voice.set_brightness(0.4);
        voice.set_damping(0.6);
        voice.trigger();
        let mut noise = WhiteNoise::default();
        let mut early = 0.0_f32;
        let mut late = 0.0_f32;
        for i in 0..44_100_usize {
            let sample = voice.process(false, &mut noise);
            assert!(sample.is_finite(), "sample {i}");
            if (100..2_000).contains(&i) {
                early += sample.abs();
            }
            if (40_000..42_000).contains(&i) {
                late += sample.abs();
            }
        }
        assert!(early / 1_900.0 > 0.005, "the voice sounds: {early}");
        assert!(late / 2_000.0 < early / 1_900.0, "the voice decays");
    }

    #[test]
    fn sustain_keeps_the_string_moving() {
        let rate = 44_100.0_f32;
        let mut voice = StringVoice::new(rate);
        voice.set_sustain(true);
        voice.set_frequency(55.0);
        voice.set_brightness(0.8);
        let mut noise = WhiteNoise::default();
        let mut energy = 0.0_f32;
        for _ in 0..44_100 {
            energy += voice.process(false, &mut noise).abs();
        }
        assert!(energy > 1.0, "the sustained voice keeps sounding: {energy}");
    }
}
