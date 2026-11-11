//! The filters the bass voice uses: a one-pole damping filter, a DC blocker, a
//! crossfade and the double-sampled state-variable filter from the reference.

use core::f32::consts::PI;

use super::{PI_F, fclamp, fmin};

/// One-pole low-pass / high-pass, used as the string's damping filter.
#[derive(Clone, Copy, Debug)]
pub struct OnePole {
    g: f32,
    gi: f32,
    state: f32,
}

impl OnePole {
    /// A cleared filter; set a frequency before the first sample.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            g: 0.0,
            gi: 0.0,
            state: 0.0,
        }
    }

    /// Clear the filter state.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }

    /// Set the cutoff, normalized to the sample rate and clipped at 0.497.
    pub fn set_frequency(&mut self, frequency: f32) {
        let frequency = if frequency < 0.497 { frequency } else { 0.497 };
        self.g = (PI_F * frequency).tan();
        self.gi = 1.0 / (1.0 + self.g);
    }

    /// Filter one sample.
    pub fn process(&mut self, input: f32) -> f32 {
        let low = (self.g * input + self.state) * self.gi;
        self.state = self.g * (input - low) + low;
        low
    }
}

impl Default for OnePole {
    fn default() -> Self {
        Self::new()
    }
}

/// A one-pole DC blocker.
#[derive(Clone, Copy, Debug)]
pub struct DcBlock {
    input: f32,
    output: f32,
    gain: f32,
}

impl DcBlock {
    /// A blocker for `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            gain: 1.0 - 10.0 / sample_rate,
        }
    }

    /// Block the DC of one sample.
    pub fn process(&mut self, input: f32) -> f32 {
        let output = input - self.input + self.gain * self.output;
        self.output = output;
        self.input = input;
        output
    }
}

/// A two-input linear crossfade.
#[derive(Clone, Copy, Debug)]
pub struct CrossFade {
    position: f32,
}

impl CrossFade {
    /// A linear crossfade centred at 0.5.
    #[must_use]
    pub const fn new() -> Self {
        Self { position: 0.5 }
    }

    /// Move the crossfade position, 0..1.
    pub fn set_position(&mut self, position: f32) {
        self.position = position;
    }

    /// Mix the two inputs at the current position.
    #[must_use]
    pub fn process(&self, first: f32, second: f32) -> f32 {
        let position = self.position;
        first * (1.0 - position) + second * position
    }
}

impl Default for CrossFade {
    fn default() -> Self {
        Self::new()
    }
}

/// The double-sampled, stable state-variable filter.
#[derive(Clone, Copy, Debug)]
pub struct Svf {
    sample_rate: f32,
    resonance: f32,
    drive: f32,
    pre_drive: f32,
    frequency: f32,
    damping: f32,
    notch: f32,
    low: f32,
    high: f32,
    band: f32,
    input: f32,
    out_low: f32,
    out_high: f32,
    out_band: f32,
    out_peak: f32,
    out_notch: f32,
    max_frequency: f32,
}

impl Svf {
    /// A filter at the reference's default cutoff, medium resonance and half
    /// drive.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            resonance: 0.5,
            drive: 0.5,
            pre_drive: 0.5,
            frequency: 0.25,
            damping: 0.0,
            notch: 0.0,
            low: 0.0,
            high: 0.0,
            band: 0.0,
            input: 0.0,
            out_low: 0.0,
            out_high: 0.0,
            out_band: 0.0,
            out_peak: 0.0,
            out_notch: 0.0,
            max_frequency: sample_rate / 3.0,
        }
    }

    /// Run one sample through the filter, updating every output.
    pub fn process(&mut self, input: f32) {
        self.input = input;
        // First pass.
        self.notch = self.input - self.damping * self.band;
        self.low += self.frequency * self.band;
        self.high = self.notch - self.low;
        self.band =
            self.frequency * self.high + self.band - self.drive * self.band * self.band * self.band;
        self.out_low = 0.5 * self.low;
        self.out_high = 0.5 * self.high;
        self.out_band = 0.5 * self.band;
        self.out_peak = 0.5 * (self.low - self.high);
        self.out_notch = 0.5 * self.notch;
        // Second pass.
        self.notch = self.input - self.damping * self.band;
        self.low += self.frequency * self.band;
        self.high = self.notch - self.low;
        self.band =
            self.frequency * self.high + self.band - self.drive * self.band * self.band * self.band;
        // Average the two passes.
        self.out_low += 0.5 * self.low;
        self.out_high += 0.5 * self.high;
        self.out_band += 0.5 * self.band;
        self.out_peak += 0.5 * (self.low - self.high);
        self.out_notch += 0.5 * self.notch;
    }

    /// Set the cutoff in Hz, clipped to `1e-6 .. sample_rate / 3`.
    pub fn set_frequency(&mut self, frequency: f32) {
        let frequency = fclamp(frequency, 1.0e-6, self.max_frequency);
        // Doubled because the filter is double-sampled.
        self.frequency = 2.0 * (PI * fmin(0.25, frequency / (self.sample_rate * 2.0))).sin();
        self.recalculate_damping();
    }

    /// Set the resonance, 0..1.
    pub fn set_resonance(&mut self, resonance: f32) {
        self.resonance = fclamp(resonance, 0.0, 1.0);
        self.recalculate_damping();
        self.drive = self.pre_drive * self.resonance;
    }

    /// Set the drive, 0..1 after the reference's 0.1 scale.
    pub fn set_drive(&mut self, drive: f32) {
        let drive = fclamp(drive * 0.1, 0.0, 1.0);
        self.pre_drive = drive;
        self.drive = self.pre_drive * self.resonance;
    }

    fn recalculate_damping(&mut self) {
        self.damping = fmin(
            2.0 * (1.0 - self.resonance.powf(0.25)),
            fmin(2.0, 2.0 / self.frequency - self.frequency * 0.5),
        );
    }

    /// The low-pass output of the last sample.
    #[must_use]
    pub const fn low_pass(&self) -> f32 {
        self.out_low
    }
}

#[cfg(test)]
mod tests {
    use super::{CrossFade, DcBlock, OnePole, Svf};

    #[test]
    fn one_pole_charges_towards_a_step() {
        let mut filter = OnePole::new();
        filter.set_frequency(0.1);
        let mut value = 0.0;
        for _ in 0..200 {
            value = filter.process(1.0);
        }
        assert!((0.9..=1.0).contains(&value), "{value}");
    }

    #[test]
    fn dc_block_removes_a_constant_offset() {
        let mut blocker = DcBlock::new(44_100.0);
        let mut last = 0.0;
        for _ in 0..44_100 {
            last = blocker.process(1.0);
        }
        assert!(last.abs() < 0.01, "{last}");
    }

    #[test]
    fn crossfade_stays_in_range_and_is_centred() {
        let mut fade = CrossFade::new();
        assert!((fade.process(1.0, 0.0) - 0.5).abs() < 1e-6);
        fade.set_position(0.0);
        assert!((fade.process(1.0, 0.0) - 1.0).abs() < 1e-6);
        fade.set_position(1.0);
        assert!((fade.process(1.0, 0.0)).abs() < 1e-6);
    }

    /// Total energy of a sine through a fresh filter, after settling.
    fn sine_energy(hz: f32) -> f32 {
        let mut filter = Svf::new(44_100.0);
        filter.set_frequency(200.0);
        filter.set_resonance(0.05);
        let mut energy = 0.0;
        for i in 0..8192 {
            let t = i as f32 / 44_100.0;
            filter.process((2.0 * core::f32::consts::PI * hz * t).sin());
            if i >= 4096 {
                energy += filter.low_pass() * filter.low_pass();
            }
        }
        energy
    }

    #[test]
    fn svf_low_pass_passes_a_low_tone_and_stops_a_high_one() {
        let low = sine_energy(100.0);
        let high = sine_energy(8_000.0);
        assert!(low > 50.0 * high, "low {low} high {high}");
    }
}
