//! The Karplus-Strong string: a comb filter with a damping one-pole, a DC
//! blocker, a dispersion all-pass and a curved-bridge non-linearity.
//!
//! Port of the reference's `String`, keeping its arithmetic order.  The two
//! places the reference reaches for `rand()` (the dispersion noise) now take the
//! caller's [`Noise`], so a render is reproducible.

use super::delay_line::DelayLine;
use super::filters::{CrossFade, DcBlock, OnePole};
use super::{Noise, ONE_TWELFTH, TWO_PI, fclamp, fmin, fonepole};
use crate::num::f32_to_usize;

/// Longest delay of the string line, in samples.
const STRING_LINE: usize = 1024;
/// Longest delay of the stretch (dispersion) line, in samples.
const STRETCH_LINE: usize = STRING_LINE / 4;

/// A plucked string.
#[derive(Clone)]
pub struct String {
    line: DelayLine<STRING_LINE>,
    stretch: DelayLine<STRETCH_LINE>,
    frequency: f32,
    non_linearity: f32,
    brightness: f32,
    damping: f32,
    sample_rate: f32,
    damping_filter: OnePole,
    dc_blocker: DcBlock,
    crossfade: CrossFade,
    dispersion_noise: f32,
    curved_bridge: f32,
    src_phase: f32,
    out_sample: [f32; 2],
}

impl String {
    /// A string tuned to 440 Hz at the reference's default settings.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut string = Self {
            line: DelayLine::new(),
            stretch: DelayLine::new(),
            frequency: 0.0,
            non_linearity: 0.5,
            brightness: 0.5,
            damping: 0.5,
            sample_rate,
            damping_filter: OnePole::new(),
            dc_blocker: DcBlock::new(sample_rate),
            crossfade: CrossFade::new(),
            dispersion_noise: 0.0,
            curved_bridge: 0.0,
            src_phase: 0.0,
            out_sample: [0.0; 2],
        };
        string.reset();
        string.set_frequency(440.0);
        string.set_damping(0.8);
        string.set_non_linearity(0.1);
        string.set_brightness(0.5);
        string
    }

    /// Clear the string and its filters.
    pub fn reset(&mut self) {
        self.line.reset();
        self.stretch.reset();
        self.damping_filter.reset();
        self.dc_blocker = DcBlock::new(self.sample_rate);
        self.dispersion_noise = 0.0;
        self.curved_bridge = 0.0;
        self.out_sample = [0.0; 2];
        self.src_phase = 0.0;
    }

    /// Excite the string and return the next sample.
    pub fn process(&mut self, input: f32, noise: &mut impl Noise) -> f32 {
        if self.non_linearity <= 0.0 {
            self.non_linearity *= -1.0;
            let result = self.process_internal(input, noise, false);
            self.non_linearity *= -1.0;
            result
        } else {
            self.process_internal(input, noise, true)
        }
    }

    /// Tune the string.
    pub fn set_frequency(&mut self, frequency: f32) {
        self.frequency = fclamp(frequency / self.sample_rate, 0.0, 0.25);
    }

    /// Set the non-linearity: negative is a curved bridge, positive is
    /// dispersion.
    pub fn set_non_linearity(&mut self, amount: f32) {
        self.non_linearity = fclamp(amount, 0.0, 1.0);
    }

    /// Set the overall brightness, 0..1.
    pub fn set_brightness(&mut self, brightness: f32) {
        self.brightness = fclamp(brightness, 0.0, 1.0);
    }

    /// Set the decay time, 0..1.
    pub fn set_damping(&mut self, damping: f32) {
        self.damping = fclamp(damping, 0.0, 1.0);
    }

    fn process_internal(&mut self, input: f32, noise: &mut impl Noise, dispersion: bool) -> f32 {
        let mut brightness = self.brightness;
        let mut delay = fclamp(1.0 / self.frequency, 4.0, STRING_LINE as f32 - 4.0);

        // Below 11.7 Hz the line is too short, so the string is upsampled with a
        // plain linear interpolator; the corner case is not worth more.
        let mut src_ratio = delay * self.frequency;
        if src_ratio >= 0.9999 {
            self.src_phase = 1.0;
            src_ratio = 1.0;
        }

        let mut damping_cutoff = fmin(
            12.0 + self.damping * self.damping * 60.0 + brightness * 24.0,
            84.0,
        );
        let mut damping_f = fmin(
            self.frequency * 2.0_f32.powf(damping_cutoff * ONE_TWELFTH),
            0.499,
        );

        // Crossfade to infinite decay.
        if self.damping >= 0.95 {
            let to_infinite = 20.0 * (self.damping - 0.95);
            brightness += to_infinite * (1.0 - brightness);
            damping_f += to_infinite * (0.4999 - damping_f);
            damping_cutoff += to_infinite * (128.0 - damping_cutoff);
        }
        self.damping_filter.set_frequency(damping_f);

        let ratio = 2.0_f32.powf(damping_cutoff * ONE_TWELFTH);
        let damping_compensation = 1.0 - 2.0 * (1.0 / ratio).atan() / TWO_PI;

        let stretch_point = self.non_linearity * (2.0 - self.non_linearity) * 0.225;
        let mut stretch_correction = (160.0 / self.sample_rate) * delay;
        stretch_correction = fclamp(stretch_correction, 1.0, 2.1);

        let noise_amount_sqrt = if self.non_linearity > 0.75 {
            4.0 * (self.non_linearity - 0.75)
        } else {
            0.0
        };
        let noise_amount = noise_amount_sqrt * noise_amount_sqrt * 0.1;
        let noise_filter = 0.06 + 0.94 * brightness * brightness;

        let bridge_curving_sqrt = self.non_linearity;
        let bridge_curving = bridge_curving_sqrt * bridge_curving_sqrt * 0.01;

        let ap_gain = -0.618 * self.non_linearity / (0.15 + self.non_linearity.abs());

        self.src_phase += src_ratio;
        if self.src_phase > 1.0 {
            self.src_phase -= 1.0;
            delay *= damping_compensation;

            if dispersion {
                let sample = noise.unit() - 0.5;
                fonepole(&mut self.dispersion_noise, sample, noise_filter);
                delay *= 1.0 + self.dispersion_noise * noise_amount;
            } else {
                delay *= 1.0 - self.curved_bridge * bridge_curving;
            }

            let mut sample = if dispersion {
                let ap_delay = delay * stretch_point;
                let main_delay =
                    delay - ap_delay * (0.408 - stretch_point * 0.308) * stretch_correction;
                if ap_delay >= 4.0 && main_delay >= 4.0 {
                    let read = self.line.read(main_delay);
                    self.stretch.allpass(read, f32_to_usize(ap_delay), ap_gain)
                } else {
                    self.line.read_hermite(delay)
                }
            } else {
                self.line.read_hermite(delay)
            };

            if !dispersion {
                let value = sample.abs() - 0.025;
                let sign = if sample > 0.0 { 1.0 } else { -1.5 };
                self.curved_bridge = (value.abs() + value) * sign;
            }

            sample += input;
            sample = fclamp(sample, -20.0, 20.0);
            sample = self.dc_blocker.process(sample);
            sample = self.damping_filter.process(sample);
            self.line.write(sample);

            self.out_sample[1] = self.out_sample[0];
            self.out_sample[0] = sample;
        }

        self.crossfade.set_position(self.src_phase);
        self.crossfade
            .process(self.out_sample[1], self.out_sample[0])
    }
}

#[cfg(test)]
mod tests {
    use super::String;
    use crate::dsp::WhiteNoise;

    #[test]
    fn a_plucked_string_decays_at_the_requested_pitch() {
        let rate = 44_100.0_f32;
        let mut string = String::new(rate);
        string.set_frequency(110.0);
        string.set_brightness(0.6);
        string.set_damping(0.5);
        let mut noise = WhiteNoise::default();
        let mut samples = Vec::new();
        for i in 0..44_100_usize {
            let input = if i == 0 { 1.0 } else { 0.0 };
            samples.push(string.process(input, &mut noise));
        }
        // Loud at the start, quiet after half a second, and periodic at 110 Hz.
        let early: f32 = samples[100..2_000].iter().map(|s| s.abs()).sum::<f32>() / 1_900.0;
        let late: f32 = samples[30_000..31_000].iter().map(|s| s.abs()).sum::<f32>() / 1_000.0;
        assert!(early > 1e-3, "the pluck is audible: {early}");
        assert!(late < early, "the string decays: {late} vs {early}");
        // Autocorrelation over the steady part: the waveform is rich in
        // harmonics, so counting zero crossings would overstate the pitch.
        let window = &samples[2_000..6_000];
        let mut best_lag = 0;
        let mut best = f32::MIN;
        for lag in 100..800 {
            let correlation: f32 = window
                .iter()
                .zip(&samples[2_000 + lag..4_000 + lag])
                .map(|(a, b)| a * b)
                .sum();
            if correlation > best {
                best = correlation;
                best_lag = lag;
            }
        }
        let hertz = rate / best_lag as f32;
        assert!(
            (hertz - 110.0).abs() < 3.0,
            "pitch {hertz} (lag {best_lag})"
        );
    }
}
