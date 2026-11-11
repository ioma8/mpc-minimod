//! The non-bass synth characters: piano, pads, lead, bells, strings, voice and
//! organ.
//!
//! They share one small oscillator/envelope engine.  The bass keeps its
//! Karplus-Strong model in [`crate::bass`]; this module supplies the other seven
//! characters and their preview voices.

use crate::bass::{BassError, Kind, Pluck};
use crate::dsp::filters::Svf;
use crate::dsp::{WhiteNoise, fclamp, fmin};
use crate::num::f32_to_usize;
use core::f32::consts::TAU;

/// Voices the preview mixes.
const VOICES: usize = 4;
/// C2, the render root.
const NOTE_HZ: f32 = 65.406_39;
/// Peak the rendered note is normalised to.
const OUTPUT_PEAK: f32 = 0.86;

/// The oscillator/envelope voice used by every non-bass character.
#[derive(Clone)]
struct Voice {
    kind: Kind,
    rate: f32,
    freq: f32,
    age: usize,
    phase: [f32; 3],
    filter: Svf,
    params: [f32; 3],
    velocity: f32,
}

impl Voice {
    fn new(kind: Kind, rate: f32, freq: f32, params: [f32; 3], velocity: f32) -> Self {
        let mut voice = Self {
            kind,
            rate,
            freq,
            age: 0,
            phase: [0.0; 3],
            filter: Svf::new(rate),
            params,
            velocity,
        };
        voice.filter.set_resonance(0.08);
        voice.filter.set_drive(0.08);
        voice.set_filter();
        voice
    }

    fn set_filter(&mut self) {
        let p = self.params;
        let cutoff = match self.kind {
            Kind::Pads => 500.0 * 18.0_f32.powf(p[1]),
            Kind::Lead => 700.0 * 20.0_f32.powf(0.5 * p[0] + 0.5 * p[2]),
            Kind::Strings => 600.0 * 18.0_f32.powf(p[1]),
            Kind::Voice => 650.0 * 18.0_f32.powf(p[2]),
            Kind::Organ => 800.0 * 18.0_f32.powf(p[2]),
            Kind::Piano => 900.0 * 16.0_f32.powf(p[2]),
            Kind::Bells | Kind::Bass => 12_000.0,
        };
        self.filter.set_frequency(fmin(cutoff, self.rate * 0.45));
    }

    fn envelope(&self) -> f32 {
        let t = self.age as f32 / self.rate;
        let duration = self.kind.duration();
        let attack = match self.kind {
            Kind::Pads => 0.04 + self.params[0] * 0.60,
            Kind::Strings => 0.03 + self.params[0] * 0.35,
            Kind::Voice => 0.06,
            Kind::Piano | Kind::Bells => 0.004,
            _ => 0.012,
        };
        let release = match self.kind {
            Kind::Pads | Kind::Strings => 0.30,
            Kind::Bells => 0.50,
            _ => 0.10,
        };
        let a = (t / attack).min(1.0);
        let r = if t > duration - release {
            ((duration - t) / release).max(0.0)
        } else {
            1.0
        };
        a * r
    }

    fn step_phase(&mut self, index: usize, ratio: f32) -> f32 {
        let value = self.phase[index] + self.freq * ratio / self.rate;
        self.phase[index] = value.fract();
        self.phase[index]
    }

    fn process(&mut self) -> f32 {
        let t = self.age as f32 / self.rate;
        let p = self.params;
        let raw = match self.kind {
            Kind::Bass => 0.0,
            Kind::Piano => {
                let phase = self.step_phase(0, 1.0);
                let mut value = 0.0;
                for harmonic in 1..=6 {
                    let weight = if harmonic == 1 {
                        1.0
                    } else {
                        p[0].powi(harmonic) / harmonic as f32
                    };
                    value += (TAU * phase * harmonic as f32).sin() * weight;
                }
                value * (-t / (0.22 + p[1] * 0.9)).exp()
            }
            Kind::Pads => {
                let width = p[2] * 0.02;
                let mut value = 0.0;
                for (index, ratio) in [1.0 - width, 1.0, 1.0 + width].iter().enumerate() {
                    let phase = self.step_phase(index, *ratio);
                    value += 1.0 - 4.0 * (phase - 0.5).abs();
                }
                value / 3.0
            }
            Kind::Lead => {
                let phase = self.step_phase(0, 1.0);
                let phase2 = self.step_phase(1, 1.0);
                let saw = 2.0 * phase - 1.0;
                let square = if phase2 < 0.5 { 1.0 } else { -1.0 };
                (saw + square * p[1] * 0.5) * 0.7 * (-t / 0.55).exp()
            }
            Kind::Bells => {
                let phase = self.step_phase(0, 1.0);
                let modulator = self.step_phase(1, 1.0 + p[2] * 6.0);
                let index = 0.5 + p[0] * 6.0;
                (TAU * phase + index * (TAU * modulator).sin()).sin()
                    * (-t / (0.16 + p[1] * 1.0)).exp()
            }
            Kind::Strings => {
                let width = p[2] * 0.012;
                let vibrato = (TAU * 5.0 * t).sin() * width;
                let mut value = 0.0;
                for (index, ratio) in [1.0 - width, 1.0, 1.0 + width].iter().enumerate() {
                    let phase = self.step_phase(index, *ratio + vibrato);
                    value += 2.0 * phase - 1.0;
                }
                value / 3.0
            }
            Kind::Voice => {
                let vowel = p[0];
                let ratios = [4.0 + vowel * 2.0, 7.0 + vowel * 5.0, 11.0 + vowel * 6.0];
                let mut value = 0.0;
                for (index, ratio) in ratios.iter().enumerate() {
                    let phase = self.step_phase(index, *ratio);
                    let weight = [1.0, 0.5 + p[1] * 0.3, 0.2 + p[1] * 0.2][index];
                    value += (TAU * phase).sin() * weight;
                }
                value * 0.6
            }
            Kind::Organ => {
                let phase = self.step_phase(0, 1.0);
                let mut value = 0.0;
                for (harmonic, weight) in [
                    (1.0, 1.0),
                    (2.0, p[0]),
                    (3.0, p[0] * 0.7),
                    (4.0, 0.35 * (0.3 + p[1])),
                    (6.0, 0.20 * (0.3 + p[1])),
                    (8.0, 0.12 * (0.3 + p[1])),
                ] {
                    value += (TAU * phase * harmonic).sin() * weight;
                }
                value * 0.45
            }
        };
        self.filter.process(raw);
        let value = self.filter.low_pass() * self.envelope() * self.velocity;
        self.age = self.age.saturating_add(1);
        value
    }
}

/// Render one non-bass character as a centred stereo note.
///
/// # Errors
///
/// Returns [`BassError`] for an unsupported rate or a silent result.
pub fn render(kind: Kind, rate: u32, params: [f32; 3]) -> Result<Pluck, BassError> {
    if kind == Kind::Bass {
        return Err(BassError::UnsupportedKind);
    }
    if !(8_000..=192_000).contains(&rate) {
        return Err(BassError::UnsupportedRate);
    }
    let params = [
        fclamp(params[0], 0.0, 1.0),
        fclamp(params[1], 0.0, 1.0),
        fclamp(params[2], 0.0, 1.0),
    ];
    let rate_f = rate as f32;
    let frames = f32_to_usize(rate_f * kind.duration());
    let mut voice = Voice::new(kind, rate_f, NOTE_HZ, params, 0.9);
    let mut audio = vec![0.0_f32; frames * 2];
    let mut peak = 0.0_f32;
    for index in 0..frames {
        let value = voice.process();
        audio[2 * index] = value;
        audio[2 * index + 1] = value;
        peak = peak.max(value.abs());
    }
    if peak < 1e-8 {
        return Err(BassError::Silent);
    }
    let gain = OUTPUT_PEAK / peak;
    for value in &mut audio {
        *value *= gain;
    }
    Ok(Pluck { audio, frames })
}

/// The oscillator characters' preview voices; only the audio writer thread
/// touches them.
pub(crate) struct SynthBank {
    rate: u32,
    kind: Kind,
    voices: [Voice; VOICES],
    active: [bool; VOICES],
    cursor: usize,
}

impl SynthBank {
    /// Four voices of `kind` at `rate`.
    pub(crate) fn new(kind: Kind, rate: u32, params: [f32; 3]) -> Self {
        let rate_f = rate as f32;
        Self {
            rate,
            kind,
            voices: core::array::from_fn(|_| Voice::new(kind, rate_f, NOTE_HZ, params, 0.5)),
            active: [false; VOICES],
            cursor: 0,
        }
    }

    /// Strike one pad on the next free voice.
    pub(crate) fn trigger(&mut self, pad: usize, velocity: f32, params: [f32; 3]) {
        let rate_f = self.rate as f32;
        let index = self.cursor % VOICES;
        self.cursor = self.cursor.wrapping_add(1);
        let semitones = pad as f32 / 12.0;
        self.voices[index] = Voice::new(
            self.kind,
            rate_f,
            NOTE_HZ * 2.0_f32.powf(semitones),
            params,
            0.55 * fclamp(velocity, 0.0, 1.0),
        );
        self.active[index] = true;
    }

    /// One frame of every active voice.
    pub(crate) fn frame(&mut self, _noise: &mut WhiteNoise, _rate: u32) -> f32 {
        let mut sample = 0.0;
        for index in 0..VOICES {
            if !self.active[index] {
                continue;
            }
            let voice = &mut self.voices[index];
            if voice.age as f32 >= self.kind.duration() * self.rate as f32 {
                self.active[index] = false;
                continue;
            }
            sample += voice.process();
        }
        fclamp(sample, -0.85, 0.85)
    }

    /// Silence every voice.
    pub(crate) fn stop(&mut self) {
        self.active = [false; VOICES];
    }
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::bass::{Kind, Pluck};
    use crate::num::f32_to_usize;

    fn spectrum_energy(pluck: &Pluck) -> f32 {
        pluck.audio.iter().map(|value| value.abs()).sum()
    }

    #[test]
    fn every_non_bass_character_renders_at_its_declared_length() {
        for kind in Kind::ALL {
            if kind == Kind::Bass {
                continue;
            }
            let pluck = render(kind, 44_100, kind.defaults()).expect("renders");
            assert_eq!(pluck.frames, f32_to_usize(44_100.0 * kind.duration()));
            assert_eq!(pluck.audio.len(), pluck.frames * 2);
            assert!(spectrum_energy(&pluck) > 1.0, "{} is silent", kind.name());
            let peak = pluck
                .audio
                .iter()
                .fold(0.0_f32, |peak, value| peak.max(value.abs()));
            assert!((peak - 0.86).abs() < 1e-3, "{} peak {peak}", kind.name());
        }
    }

    #[test]
    fn a_pad_note_reaches_the_non_bass_preview() {
        let _serial = crate::preview::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut left = [0.0_f32; 512];
        let mut right = [0.0_f32; 512];
        crate::bass::preview_set_kind(Kind::Pads);
        crate::bass::preview_enable(false);
        crate::bass::preview_enable(true);
        crate::bass::preview_set_params(Kind::Pads.defaults());
        // The page opening resets the synth; consume that reset before a note,
        // as the audio thread would on its next block.
        // SAFETY: the empty slice carries no writable memory.
        let _ = unsafe { crate::bass::preview_mix_slices(&mut [], 0, 44_100) };
        crate::bass::preview_note(0, 1.0);
        // SAFETY: both slices hold 512 samples and the preview tests are
        // serialised by the bass module's mutex.
        let peak = unsafe {
            crate::bass::preview_mix_slices(&mut [&mut left[..], &mut right[..]], 512, 44_100)
        };
        crate::bass::preview_enable(false);
        crate::bass::preview_reset();
        crate::bass::preview_set_kind(Kind::Bass);
        assert!(peak > 1e-4, "non-bass preview was silent: {peak}");
    }

    #[test]
    fn every_knob_changes_its_character() {
        for kind in Kind::ALL {
            if kind == Kind::Bass {
                continue;
            }
            let base = render(kind, 44_100, kind.defaults()).expect("base");
            for knob in 0..3 {
                let mut params = kind.defaults();
                params[knob] = if params[knob] < 0.5 { 1.0 } else { 0.0 };
                let changed = render(kind, 44_100, params).expect("changed");
                let difference = base
                    .audio
                    .iter()
                    .zip(&changed.audio)
                    .filter(|(a, b)| (*a - *b).abs() > 1e-6)
                    .count();
                assert!(
                    difference > base.audio.len() / 10,
                    "{} K{} does not change the sound",
                    kind.name(),
                    knob + 1
                );
            }
        }
    }

    #[test]
    fn the_encoder_wraps_in_both_directions() {
        assert_eq!(Kind::Bass.step(-1), Kind::Organ);
        assert_eq!(Kind::Organ.step(1), Kind::Bass);
        assert_eq!(Kind::Lead.name(), "LEAD");
        assert_eq!(Kind::Strings.knobs(), ["ATTACK", "BRIGHT", "WIDTH"]);
    }
}
