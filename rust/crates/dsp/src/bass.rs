//! The bass synth: a rendered C2 pluck, and the four-voice realtime preview.
//!
//! Both use the same voice ([`StringVoice`]) the reference uses.  The rendered
//! pluck is a short 0.75 s note with a fast gated tail, so it sounds like a
//! punchy bass rather than a sustained pad; the preview tracks the three knobs
//! and the sixteen pads for live playing.
//!
//! The preview is a process singleton because it is driven by the audio writer
//! hook, which exists once.  Its parameters and pending pad strikes are atomics,
//! so the pad thread never touches the synth itself.

use crate::dsp::filters::Svf;
use crate::dsp::string_voice::StringVoice;
use crate::dsp::{Noise, WhiteNoise, fclamp};
use crate::num::f32_to_usize;

/// The eight synth characters the control page can select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Short Karplus-Strong bass.
    Bass,
    /// Bright, decaying piano.
    Piano,
    /// Slow, detuned pad.
    Pads,
    /// Focused lead with bite.
    Lead,
    /// FM bell.
    Bells,
    /// Bowed string ensemble.
    Strings,
    /// Formant voice.
    Voice,
    /// Additive drawbar organ.
    Organ,
}

impl Kind {
    /// All kinds, in encoder order.
    pub const ALL: [Self; 8] = [
        Self::Bass,
        Self::Piano,
        Self::Pads,
        Self::Lead,
        Self::Bells,
        Self::Strings,
        Self::Voice,
        Self::Organ,
    ];

    /// The name shown on the control page.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bass => "BASS",
            Self::Piano => "PIANO",
            Self::Pads => "PADS",
            Self::Lead => "LEAD",
            Self::Bells => "BELLS",
            Self::Strings => "STRINGS",
            Self::Voice => "VOICE",
            Self::Organ => "ORGAN",
        }
    }

    /// The three knob labels for this character.
    #[must_use]
    pub const fn knobs(self) -> [&'static str; 3] {
        match self {
            Self::Bass => ["BRIGHT", "DECAY", "FILTER"],
            Self::Piano => ["BRIGHT", "DECAY", "TONE"],
            Self::Pads | Self::Strings => ["ATTACK", "BRIGHT", "WIDTH"],
            Self::Lead => ["BRIGHT", "BITE", "FILTER"],
            Self::Bells => ["BRIGHT", "DECAY", "RATIO"],
            Self::Voice => ["VOWEL", "BRIGHT", "FILTER"],
            Self::Organ => ["DRAWBAR", "BRIGHT", "FILTER"],
        }
    }

    /// Default knob positions for this character.
    #[must_use]
    pub const fn defaults(self) -> [f32; 3] {
        match self {
            Self::Bass => [0.52, 0.25, 0.56],
            Self::Piano => [0.58, 0.35, 0.62],
            Self::Pads => [0.35, 0.48, 0.45],
            Self::Lead => [0.55, 0.42, 0.62],
            Self::Bells => [0.50, 0.35, 0.45],
            Self::Strings => [0.25, 0.46, 0.50],
            Self::Voice => [0.35, 0.50, 0.58],
            Self::Organ => [0.55, 0.50, 0.60],
        }
    }

    /// The rendered note length, in seconds.
    #[must_use]
    pub const fn duration(self) -> f32 {
        match self {
            Self::Bass => 0.75,
            Self::Piano => 1.2,
            Self::Pads => 1.4,
            Self::Lead => 0.8,
            Self::Bells => 1.5,
            Self::Strings => 1.3,
            Self::Voice => 1.0,
            Self::Organ => 0.9,
        }
    }

    /// The index in [`Kind::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The kind at an encoder index, wrapping.
    #[must_use]
    pub fn from_index(index: i32) -> Self {
        let count = i32::try_from(Self::ALL.len()).unwrap_or(1);
        let wrapped = index.rem_euclid(count);
        Self::ALL[usize::try_from(wrapped).unwrap_or(0)]
    }

    /// The next kind in the requested direction.
    ///
    /// The addition is saturating, so an extreme delta cannot overflow the
    /// encoder position before it wraps.
    #[must_use]
    pub fn step(self, delta: i32) -> Self {
        let index = i64::from(delta).saturating_add(i64::try_from(self.index()).unwrap_or(0));
        let count = i64::try_from(Self::ALL.len()).unwrap_or(1);
        let wrapped = i32::try_from(index.rem_euclid(count)).unwrap_or(0);
        Self::from_index(wrapped)
    }
}

/// Pads the preview can play.
pub const PADS: usize = 16;
/// Voices the preview mixes.
const VOICES: usize = 4;
/// The rendered pluck's root: C2, MIDI 36.  Written as bits because it has to be
/// the reference's `65.4063913f` exactly: one ulp here detunes every sample.
const NOTE_HZ: f32 = f32::from_bits(0x4282_d013);
/// Where the rendered note starts its final decay, in seconds.
const GATE: f32 = 0.15;
/// Drive/excitation is a fixed character now that the second knob is DECAY.
const DRIVE: f32 = 0.35;
/// Peak the rendered pluck is normalised to.
const OUTPUT_PEAK: f32 = 0.86;
/// How long a preview voice may sound, in seconds. The preview is an audition,
/// not the saved sample: it decays from the first sample so a stack of pads can
/// never turn into a sustained drone.
const PREVIEW_LENGTH: f32 = 0.7;

/// Peak the preview mix is allowed to reach before it is clamped.
const PREVIEW_PEAK: f32 = 0.85;
/// Nominal rate the engine is built at before the device reports its own.
pub(crate) const NOMINAL_RATE: u32 = 44_100;

/// A rendered pluck, interleaved stereo.
#[derive(Clone, Debug)]
pub struct Pluck {
    /// Interleaved stereo frames.
    pub audio: Vec<f32>,
    /// Length in frames; `audio.len() == frames * 2`.
    pub frames: usize,
}

/// Why a pluck could not be rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BassError {
    /// The sample rate is outside the supported 8 kHz to 192 kHz range.
    UnsupportedRate,
    /// The synth produced silence, which means a parameter is unusable.
    Silent,
    /// The requested character is not handled by this renderer.
    UnsupportedKind,
}

impl core::fmt::Display for BassError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedRate => f.write_str("unsupported sample rate"),
            Self::Silent => f.write_str("the synth produced silence"),
            Self::UnsupportedKind => f.write_str("unsupported synth character"),
        }
    }
}

impl std::error::Error for BassError {}

/// Render the C2 pluck with the default noise source.
///
/// # Errors
///
/// Returns [`BassError`] for an unsupported rate or a silent result.
pub fn render(rate: u32, harmonics: f32, decay: f32, cutoff: f32) -> Result<Pluck, BassError> {
    render_with(rate, harmonics, decay, cutoff, &mut WhiteNoise::default())
}

/// Render one synth character.
///
/// The bass keeps its dedicated string model; the other characters live in
/// [`crate::synth`].
///
/// # Errors
///
/// Returns [`BassError`] when the synth cannot render.
pub fn render_kind(kind: Kind, rate: u32, params: [f32; 3]) -> Result<Pluck, BassError> {
    if kind == Kind::Bass {
        render(rate, params[0], params[1], params[2])
    } else {
        crate::synth::render(kind, rate, params)
    }
}

/// Render the C2 pluck with an explicit noise source, for the golden tests.
///
/// # Errors
///
/// Returns [`BassError`] for an unsupported rate or a silent result.
pub fn render_with(
    rate: u32,
    harmonics: f32,
    decay: f32,
    cutoff: f32,
    noise: &mut impl Noise,
) -> Result<Pluck, BassError> {
    if !(8_000..=192_000).contains(&rate) {
        return Err(BassError::UnsupportedRate);
    }
    let rate_f = rate as f32;
    let frames = f32_to_usize(rate_f * Kind::Bass.duration());
    let harmonics = clamp01(harmonics);
    let decay = clamp01(decay);
    let cutoff = clamp01(cutoff);

    let mut string = StringVoice::new(rate_f);
    string.set_sustain(false);
    string.set_frequency(NOTE_HZ);
    string.set_accent(0.65 + DRIVE * 0.35);
    string.set_structure(0.25 + DRIVE * 0.6);
    string.set_brightness(0.05 + harmonics * 0.9);
    string.set_damping(0.35 + (1.0 - harmonics) * 0.55);
    string.trigger();

    let mut filter = Svf::new(rate_f);
    let cutoff_hz = 700.0 * 24.0_f32.powf(cutoff);
    filter.set_frequency(if cutoff_hz < rate_f * 0.45 {
        cutoff_hz
    } else {
        rate_f * 0.45
    });
    filter.set_resonance(0.05);
    filter.set_drive(DRIVE * 0.35);

    let mut audio = vec![0.0_f32; frames * 2];
    let gate = f32_to_usize(rate_f * GATE);
    let tail = 0.08 + decay * 0.34;
    let mut peak = 0.0_f32;
    for i in 0..frames {
        let sample = string.process(i == 0, noise);
        filter.process(sample);
        let mut value = filter.low_pass();
        if i > gate {
            let t = (i - gate) as f32 / rate_f;
            value *= (-t / tail).exp();
        }
        audio[2 * i] = value;
        audio[2 * i + 1] = value;
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

/// One preview voice.
#[derive(Clone)]
struct Voice {
    string: StringVoice,
    filter: Svf,
    age: u32,
    quiet: u32,
    level: f32,
    /// Time constant of the preview's exponential tail, in seconds.
    decay: f32,
    active: bool,
}

impl Voice {
    fn new(rate: f32) -> Self {
        Self {
            string: StringVoice::new(rate),
            filter: Svf::new(rate),
            age: 0,
            quiet: 0,
            level: 0.0,
            decay: 0.6,
            active: false,
        }
    }
}

/// The bass preview's voices; only the audio writer thread touches them.
pub(crate) struct BassBank {
    rate: u32,
    voices: [Voice; VOICES],
    cursor: usize,
}

impl BassBank {
    /// Four cleared voices at `rate`.
    pub(crate) fn new(rate: u32) -> Self {
        let rate_f = rate as f32;
        Self {
            rate,
            voices: core::array::from_fn(|_| Voice::new(rate_f)),
            cursor: 0,
        }
    }

    /// Apply the live brightness and cutoff to the voices already sounding.
    pub(crate) fn configure(&mut self, brightness: f32, cutoff: f32) {
        let rate_f = self.rate as f32;
        let cutoff_hz = 700.0 * 24.0_f32.powf(cutoff);
        let safe_cutoff = if cutoff_hz < rate_f * 0.45 {
            cutoff_hz
        } else {
            rate_f * 0.45
        };
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            voice.string.set_accent(0.65 + DRIVE * 0.35);
            voice.string.set_structure(0.25 + DRIVE * 0.6);
            voice.string.set_brightness(0.05 + brightness * 0.9);
            voice.string.set_damping(0.35 + (1.0 - brightness) * 0.55);
            voice.filter.set_frequency(safe_cutoff);
            voice.filter.set_drive(DRIVE * 0.35);
        }
    }

    /// Strike one pad on the next free voice.
    pub(crate) fn trigger(
        &mut self,
        pad: usize,
        velocity: f32,
        brightness: f32,
        decay: f32,
        cutoff: f32,
    ) {
        let rate_f = self.rate as f32;
        let index = self.cursor % VOICES;
        self.cursor = self.cursor.wrapping_add(1);
        let voice = &mut self.voices[index];
        voice.string.reset();
        let semitones = pad as f32 / 12.0;
        voice
            .string
            .set_frequency(NOTE_HZ * 2.0_f32.powf(semitones));
        voice.string.set_accent(0.65 + DRIVE * 0.35);
        voice.string.set_structure(0.25 + DRIVE * 0.6);
        voice.string.set_brightness(0.05 + brightness * 0.9);
        voice.string.set_damping(0.35 + (1.0 - brightness) * 0.55);
        voice.string.trigger();
        voice.filter = Svf::new(rate_f);
        let cutoff_hz = 700.0 * 24.0_f32.powf(cutoff);
        voice.filter.set_frequency(if cutoff_hz < rate_f * 0.45 {
            cutoff_hz
        } else {
            rate_f * 0.45
        });
        voice.filter.set_resonance(0.05);
        voice.filter.set_drive(DRIVE * 0.35);
        voice.level = 0.5 * clamp01(velocity);
        voice.decay = 0.08 + decay * 0.30;
        voice.age = 0;
        voice.quiet = 0;
        voice.active = true;
    }

    /// One frame of every active voice.
    pub(crate) fn frame(&mut self, noise: &mut WhiteNoise, rate: u32) -> f32 {
        let rate_f = rate as f32;
        let max_age = rate_f * PREVIEW_LENGTH;
        let max_quiet = rate / 3;
        let mut sample = 0.0;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            voice.filter.process(voice.string.process(false, noise));
            let t = voice.age as f32 / rate_f;
            let value = voice.filter.low_pass() * voice.level * (-t / voice.decay).exp();
            sample += value;
            if value.abs() < 1e-5 {
                voice.quiet += 1;
            } else {
                voice.quiet = 0;
            }
            voice.age += 1;
            if voice.age as f32 > max_age || voice.quiet > max_quiet {
                voice.active = false;
            }
        }
        fclamp(sample, -PREVIEW_PEAK, PREVIEW_PEAK)
    }

    /// Silence every voice.
    pub(crate) fn stop(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
        }
    }
}

// The preview's public surface lives in `crate::preview`; these names are what
// the feature and the tests already use.
pub use crate::preview::{
    enable as preview_enable, kind as preview_kind, mix_slices as preview_mix_slices,
    note as preview_note, reset as preview_reset, set_kind as preview_set_kind,
    set_params as preview_set_params,
};

/// Clamp to 0..1, treating a non-finite value as zero.
fn clamp01(value: f32) -> f32 {
    if !value.is_finite() || value < 0.0 {
        0.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BassError, Kind, PADS, Pluck, preview_enable, preview_mix_slices, preview_note, render,
        render_with,
    };
    use crate::dsp::SequenceNoise;

    #[test]
    fn the_pluck_is_stereo_centred_and_normalised() {
        let pluck = render(44_100, 0.5, 0.3, 0.6).expect("renders");
        assert_eq!(pluck.frames, 33_075);
        assert_eq!(pluck.audio.len(), pluck.frames * 2);
        let peak = pluck.audio.iter().fold(0.0_f32, |p, v| p.max(v.abs()));
        assert!((peak - 0.86).abs() < 1e-3, "peak {peak}");
        // Both channels carry the same signal.
        for frame in 0..pluck.frames {
            assert_eq!(
                pluck.audio[frame * 2].to_bits(),
                pluck.audio[frame * 2 + 1].to_bits()
            );
        }
        // The gate closed at 0.15 s, so the last tenth of a second is much
        // quieter than the body of the note.
        let tail: f32 = pluck.audio[pluck.audio.len() - 4_410..]
            .iter()
            .map(|v| v.abs())
            .sum::<f32>()
            / 4_410.0;
        let body: f32 = pluck.audio[4_410..8_820]
            .iter()
            .map(|v| v.abs())
            .sum::<f32>()
            / 4_410.0;
        assert!(tail < 0.5 * body, "tail {tail} body {body}");
    }

    #[test]
    fn the_decay_knob_changes_the_tail_not_the_note_length() {
        let short =
            render_with(44_100, 0.55, 0.0, 0.62, &mut SequenceNoise::new(7)).expect("renders");
        let long =
            render_with(44_100, 0.55, 1.0, 0.62, &mut SequenceNoise::new(7)).expect("renders");
        assert_eq!(short.frames, long.frames);
        let start = short.frames * 3 / 4;
        let energy = |pluck: &Pluck| {
            pluck.audio[start * 2..]
                .iter()
                .map(|value| value.abs())
                .sum::<f32>()
        };
        assert!(
            energy(&short) * 2.0 < energy(&long),
            "short tail {} should be well below long tail {}",
            energy(&short),
            energy(&long)
        );
    }

    #[test]
    fn the_pluck_is_deterministic_for_a_given_noise_sequence() {
        let mut first = SequenceNoise::new(1);
        let mut second = SequenceNoise::new(1);
        let a = render_with(44_100, 0.5, 0.3, 0.6, &mut first).expect("renders");
        let b = render_with(44_100, 0.5, 0.3, 0.6, &mut second).expect("renders");
        assert_eq!(a.audio.len(), b.audio.len());
        for (x, y) in a.audio.iter().zip(b.audio.iter()) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }

    #[test]
    fn every_kind_round_trips_through_its_index() {
        for (index, kind) in Kind::ALL.iter().enumerate() {
            assert_eq!(kind.index(), index, "{kind:?}");
            assert_eq!(Kind::from_index(i32::try_from(index).unwrap_or(-1)), *kind);
        }
        for index in -20..20 {
            let kind = Kind::from_index(index);
            assert_eq!(Kind::ALL[kind.index()], kind);
            assert_eq!(Kind::from_index(index).step(0), kind);
        }
        assert_eq!(Kind::Bass.step(-1), Kind::Organ);
        assert_eq!(Kind::Organ.step(1), Kind::Bass);
        assert_eq!(Kind::Bass.step(i32::MAX), Kind::Bass.step(i32::MAX - 8));
    }

    #[test]
    fn unsupported_rates_are_refused() {
        assert_eq!(
            render(100, 0.5, 0.3, 0.6).unwrap_err(),
            BassError::UnsupportedRate
        );
        assert_eq!(
            render(500_000, 0.5, 0.3, 0.6).unwrap_err(),
            BassError::UnsupportedRate
        );
    }

    /// The preview is a process singleton, so its tests take turns.
    static PREVIEW_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_preview_note_dies_within_its_envelope() {
        use super::preview_reset;
        let _serial = crate::preview::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _serial = PREVIEW_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        preview_enable(true);
        preview_reset();
        preview_note(3, 1.0);
        let rate = 44_100;
        let mut peak_late = 0.0_f32;
        // Two seconds of 128-frame blocks; the voice is capped at `PREVIEW_LENGTH`.
        for block in 0..(2 * rate / 128) {
            let mut left = [0.0_f32; 128];
            let mut right = [0.0_f32; 128];
            // SAFETY: both slices hold 128 samples and this test is serialised
            // against the other preview tests.
            let peak =
                unsafe { preview_mix_slices(&mut [&mut left[..], &mut right[..]], 128, rate) };
            if block * 128 > rate * 3 / 2 {
                peak_late = peak_late.max(peak);
            }
        }
        preview_enable(false);
        assert!(
            peak_late < 1e-3,
            "still ringing after the envelope: {peak_late}"
        );
    }

    #[test]
    fn the_preview_strikes_a_pad_and_reports_a_peak() {
        let _serial = crate::preview::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _serial = PREVIEW_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _: Pluck = render(44_100, 0.5, 0.3, 0.6).expect("renders");
        assert_eq!(PADS, 16);
        preview_enable(true);
        // Let the thread consume any reset left by a previous test before the
        // strike is queued.
        // SAFETY: the empty slice carries no writable memory.
        let _ = unsafe { preview_mix_slices(&mut [], 0, 44_100) };
        preview_note(0, 0.8);
        let mut left = vec![0.0_f32; 4_096];
        let mut right = vec![0.0_f32; 4_096];
        let frames = left.len();
        // SAFETY: both slices hold `frames` samples and this test is serialised
        // against the other preview tests.
        let peak =
            unsafe { preview_mix_slices(&mut [&mut left[..], &mut right[..]], frames, 44_100) };
        assert!(peak > 0.0, "the strike is audible: {peak}");
        assert!(left.iter().any(|v| *v != 0.0));
        assert_eq!(left, right, "both channels get the same voice mix");
        preview_enable(false);
        // SAFETY: both slices still hold `frames` samples.
        let silent =
            unsafe { preview_mix_slices(&mut [&mut left[..], &mut right[..]], frames, 44_100) };
        assert!(silent.abs() < f32::EPSILON);
    }
}
