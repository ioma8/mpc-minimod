//! The non-bass synth characters: piano, pads, lead, bells, strings, voice and
//! organ.
//!
//! They share one small oscillator/envelope engine.  The bass keeps its
//! Karplus-Strong model in [`crate::bass`]; this module supplies the other seven
//! characters and their preview voices.

use core::f32::consts::TAU;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::bass::{BassError, Kind, PADS, Pluck};
use crate::dsp::filters::Svf;
use crate::dsp::{fclamp, fmin};
use crate::num::f32_to_usize;

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

/// The preview engine, only ever touched by the audio writer thread.
struct Preview {
    rate: u32,
    kind: Kind,
    params: [f32; 3],
    voices: [Voice; VOICES],
    active: [bool; VOICES],
    cursor: usize,
    was_enabled: bool,
}

impl Preview {
    fn new() -> Self {
        let rate = 44_100;
        let rate_f = rate as f32;
        Self {
            rate,
            kind: Kind::Pads,
            params: Kind::Pads.defaults(),
            voices: core::array::from_fn(|_| {
                Voice::new(Kind::Pads, rate_f, NOTE_HZ, Kind::Pads.defaults(), 0.5)
            }),
            active: [false; VOICES],
            cursor: 0,
            was_enabled: false,
        }
    }

    fn init(&mut self, rate: u32) {
        self.rate = rate;
        let rate_f = rate as f32;
        self.voices =
            core::array::from_fn(|_| Voice::new(self.kind, rate_f, NOTE_HZ, self.params, 0.5));
        self.active = [false; VOICES];
        self.cursor = 0;
    }

    fn stop(&mut self) {
        self.active = [false; VOICES];
        for pending in &PENDING {
            pending.store(0, Ordering::Release);
        }
    }

    fn trigger(&mut self, pad: usize, velocity: f32) {
        let rate_f = self.rate as f32;
        let index = self.cursor % VOICES;
        self.cursor = self.cursor.wrapping_add(1);
        let semitones = pad as f32 / 12.0;
        self.voices[index] = Voice::new(
            self.kind,
            rate_f,
            NOTE_HZ * 2.0_f32.powf(semitones),
            self.params,
            0.55 * fclamp(velocity, 0.0, 1.0),
        );
        self.active[index] = true;
    }
}

struct PreviewCell(core::cell::UnsafeCell<Option<Preview>>);

// SAFETY: see `crate::bass`; only the audio writer thread reaches the synth.
unsafe impl Sync for PreviewCell {}

static PREVIEW: PreviewCell = PreviewCell(core::cell::UnsafeCell::new(None));
static RESET: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
static KIND: AtomicU32 = AtomicU32::new(Kind::Pads.index() as u32);
static PARAMS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
static PENDING: [AtomicU32; PADS] = [const { AtomicU32::new(0) }; PADS];

fn with_preview<R>(f: impl FnOnce(&mut Preview) -> R) -> R {
    // SAFETY: only the audio writer thread calls this.
    unsafe {
        let slot = &mut *PREVIEW.0.get();
        f(slot.get_or_insert_with(Preview::new))
    }
}

/// Turn the non-bass preview on or off.
pub fn preview_enable(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

/// Select the character; the next audio block rebuilds the voices.
pub fn preview_set_kind(kind: Kind) {
    KIND.store(kind.index() as u32, Ordering::Release);
    preview_reset();
}

/// Set the three knobs, each 0..1.
pub fn preview_set_params(params: [f32; 3]) {
    for (slot, value) in PARAMS.iter().zip(params) {
        slot.store(fclamp(value, 0.0, 1.0).to_bits(), Ordering::Release);
    }
}

/// Ask the audio thread to silence every voice.
pub fn preview_reset() {
    RESET.store(true, Ordering::Release);
}

/// Strike one pad; the audio thread picks it up.
pub fn preview_note(pad: usize, velocity: f32) {
    if velocity <= 0.0 || !velocity.is_finite() {
        return;
    }
    if let Some(slot) = PENDING.get(pad) {
        slot.store(fclamp(velocity, 0.0, 1.0).to_bits(), Ordering::Release);
    }
}

/// Mix the preview into up to two channel slices.
pub fn preview_mix_slices(channels: &mut [&mut [f32]], frames: usize, rate: u32) -> f32 {
    let mut pointers: [*mut f32; 2] = [core::ptr::null_mut(); 2];
    let count = channels.len().min(2);
    for (slot, channel) in pointers.iter_mut().zip(channels.iter_mut()).take(count) {
        *slot = channel.as_mut_ptr();
    }
    // SAFETY: every pointer is writable for `frames` samples.
    unsafe { preview_mix(&mut pointers[..count], frames, rate) }
}

/// Mix the preview into output channels and report its peak.
///
/// # Safety
///
/// Every pointer must be writable for `frames` samples.
pub unsafe fn preview_mix(channels: &mut [*mut f32], frames: usize, rate: u32) -> f32 {
    let channel_count = channels.len();
    if RESET.swap(false, Ordering::AcqRel) {
        with_preview(Preview::stop);
    }
    if !ENABLED.load(Ordering::Acquire)
        || channel_count == 0
        || frames == 0
        || !(8_000..=192_000).contains(&rate)
    {
        with_preview(|preview| {
            if preview.was_enabled {
                preview.stop();
            }
            preview.was_enabled = false;
        });
        return 0.0;
    }

    let kind = Kind::from_index(i32::try_from(KIND.load(Ordering::Acquire)).unwrap_or(0));
    let params = [
        f32::from_bits(PARAMS[0].load(Ordering::Acquire)),
        f32::from_bits(PARAMS[1].load(Ordering::Acquire)),
        f32::from_bits(PARAMS[2].load(Ordering::Acquire)),
    ];
    with_preview(|preview| {
        let params_changed = preview
            .params
            .iter()
            .zip(params)
            .any(|(old, new)| old.to_bits() != new.to_bits());
        let changed = preview.rate != rate || preview.kind != kind || params_changed;
        preview.kind = kind;
        preview.params = params;
        if changed {
            preview.init(rate);
        }
        preview.was_enabled = true;
        for (pad, pending) in PENDING.iter().enumerate() {
            let bits = pending.swap(0, Ordering::AcqRel);
            if bits != 0 {
                preview.trigger(pad, f32::from_bits(bits));
            }
        }
        let mut peak = 0.0_f32;
        for frame in 0..frames {
            let mut sample = 0.0;
            for index in 0..VOICES {
                if !preview.active[index] {
                    continue;
                }
                let voice = &mut preview.voices[index];
                if voice.age as f32 >= kind.duration() * rate as f32 {
                    preview.active[index] = false;
                    continue;
                }
                sample += voice.process();
            }
            sample = fclamp(sample, -0.85, 0.85);
            for pointer in channels.iter_mut() {
                // SAFETY: the caller promises each channel is writable.
                let channel = unsafe { core::slice::from_raw_parts_mut(*pointer, frames) };
                channel[frame] = fclamp(channel[frame] + sample, -1.0, 1.0);
            }
            peak = peak.max(sample.abs());
        }
        peak
    })
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
        let mut left = [0.0_f32; 512];
        let mut right = [0.0_f32; 512];
        crate::bass::preview_set_kind(Kind::Pads);
        crate::bass::preview_enable(false);
        super::preview_enable(true);
        super::preview_set_params(Kind::Pads.defaults());
        // The page opening resets the synth; consume that reset before a note,
        // as the audio thread would on its next block.
        let _ = crate::bass::preview_mix_slices(&mut [], 0, 44_100);
        crate::bass::preview_note(0, 1.0);
        let peak =
            crate::bass::preview_mix_slices(&mut [&mut left[..], &mut right[..]], 512, 44_100);
        super::preview_enable(false);
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
