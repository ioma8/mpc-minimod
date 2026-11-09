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

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::dsp::filters::Svf;
use crate::dsp::string_voice::StringVoice;
use crate::dsp::{Noise, WhiteNoise, fclamp};
use crate::num::f32_to_usize;

/// Pads the preview can play.
pub const PADS: usize = 16;
/// Voices the preview mixes.
const VOICES: usize = 4;
/// The rendered pluck's root: C2, MIDI 36.  Written as bits because it has to be
/// the reference's `65.4063913f` exactly: one ulp here detunes every sample.
const NOTE_HZ: f32 = f32::from_bits(0x4282_d013);
/// Rendered note length, in seconds.  Short enough to stay a bass note.
const DURATION: f32 = 0.75;
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
const NOMINAL_RATE: u32 = 44_100;

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
}

impl core::fmt::Display for BassError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedRate => f.write_str("unsupported sample rate"),
            Self::Silent => f.write_str("the synth produced silence"),
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
    let frames = f32_to_usize(rate_f * DURATION);
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

/// The preview synth's state; only the audio writer thread touches it.
struct Preview {
    rate: u32,
    voices: [Voice; VOICES],
    cursor: usize,
    was_enabled: bool,
    noise: WhiteNoise,
}

impl Preview {
    fn new() -> Self {
        let rate_f = NOMINAL_RATE as f32;
        Self {
            rate: NOMINAL_RATE,
            voices: core::array::from_fn(|_| Voice::new(rate_f)),
            cursor: 0,
            was_enabled: false,
            noise: WhiteNoise::default(),
        }
    }

    fn init(&mut self, rate: u32) {
        self.rate = rate;
        let rate_f = rate as f32;
        for voice in &mut self.voices {
            voice.string = StringVoice::new(rate_f);
            voice.string.set_sustain(false);
            voice.filter = Svf::new(rate_f);
            voice.active = false;
        }
        self.cursor = 0;
    }

    fn stop(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
        }
        for pending in &PENDING {
            pending.store(0, Ordering::Release);
        }
    }

    fn trigger(&mut self, pad: usize, velocity: f32, brightness: f32, decay: f32, cutoff: f32) {
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
}

/// The preview engine, only ever touched by the audio writer thread.
struct PreviewCell(core::cell::UnsafeCell<Option<Preview>>);

// SAFETY: the synth is only ever reached through `with_preview`, which runs on
// the audio writer thread; the pad and UI threads only use the atomics below.
// In particular `preview_enable` must not touch the synth: it is called from the
// UI thread and would race the mixer.
unsafe impl Sync for PreviewCell {}

static PREVIEW: PreviewCell = PreviewCell(core::cell::UnsafeCell::new(None));
/// A reset the audio thread has not run yet.
static RESET: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Brightness, as f32 bits.
static BRIGHTNESS: AtomicU32 = AtomicU32::new(0.52_f32.to_bits());
/// Decay, as f32 bits.
static DECAY: AtomicU32 = AtomicU32::new(0.25_f32.to_bits());
/// Filter cutoff, as f32 bits.
static CUTOFF: AtomicU32 = AtomicU32::new(0.56_f32.to_bits());
/// Velocity per pad, as f32 bits; zero means "no strike pending".
static PENDING: [AtomicU32; PADS] = [const { AtomicU32::new(0) }; PADS];

fn with_preview<R>(f: impl FnOnce(&mut Preview) -> R) -> R {
    // SAFETY: see `PreviewCell`; a preview is built on first use and never
    // removed.
    unsafe {
        let slot = &mut *PREVIEW.0.get();
        f(slot.get_or_insert_with(Preview::new))
    }
}

/// Turn the preview on or off.
///
/// Only the flag is written here: the synth belongs to the audio thread, which
/// silences every voice on its next block once the flag is off.  Reaching the
/// synth from this thread would race the mixer.
pub fn preview_enable(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

/// Set the three knobs, each 0..1.
pub fn preview_set_params(brightness: f32, decay: f32, cutoff: f32) {
    BRIGHTNESS.store(clamp01(brightness).to_bits(), Ordering::Release);
    DECAY.store(clamp01(decay).to_bits(), Ordering::Release);
    CUTOFF.store(clamp01(cutoff).to_bits(), Ordering::Release);
}

/// Strike one pad; the audio thread picks the velocity up.
pub fn preview_note(pad: usize, velocity: f32) {
    if velocity <= 0.0 || !velocity.is_finite() {
        return;
    }
    if let Some(slot) = PENDING.get(pad) {
        slot.store(clamp01(velocity).to_bits(), Ordering::Release);
    }
}

/// Ask the audio thread to silence every voice and drop pending strikes before
/// its next block, so a page that just opened never plays a stale note.
pub fn preview_reset() {
    RESET.store(true, Ordering::Release);
}

/// Mix the preview into up to two channel slices.
///
/// The slices must all be at least `frames` long; extra channels are ignored,
/// matching the stereo preview.
pub fn preview_mix_slices(channels: &mut [&mut [f32]], frames: usize, rate: u32) -> f32 {
    let mut pointers: [*mut f32; 2] = [core::ptr::null_mut(); 2];
    let count = channels.len().min(2);
    for (slot, channel) in pointers.iter_mut().zip(channels.iter_mut()).take(count) {
        *slot = channel.as_mut_ptr();
    }
    // SAFETY: every pointer addresses a slice of at least `frames` samples, and
    // the count matches the pointers filled.
    unsafe { preview_mix(&mut pointers[..count], frames, rate) }
}

/// Mix the preview into the device's output channels and report its peak.
///
/// # Safety
///
/// Every pointer in `channels` must be writable for `frames` samples, and
/// `channels` itself may be empty (which mixes nothing).
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

    let brightness = f32::from_bits(BRIGHTNESS.load(Ordering::Acquire));
    let decay = f32::from_bits(DECAY.load(Ordering::Acquire));
    let cutoff = f32::from_bits(CUTOFF.load(Ordering::Acquire));
    let rate_f = rate as f32;
    let cutoff_hz = 700.0 * 24.0_f32.powf(cutoff);
    let safe_cutoff = if cutoff_hz < rate_f * 0.45 {
        cutoff_hz
    } else {
        rate_f * 0.45
    };

    with_preview(|preview| {
        if preview.rate != rate {
            preview.init(rate);
        }
        preview.was_enabled = true;

        for (pad, pending) in PENDING.iter().enumerate() {
            let bits = pending.swap(0, Ordering::AcqRel);
            if bits != 0 {
                preview.trigger(pad, f32::from_bits(bits), brightness, decay, cutoff);
            }
        }
        for voice in &mut preview.voices {
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

        let mut noise = preview.noise;
        let max_age = rate_f * PREVIEW_LENGTH;
        let max_quiet = rate / 3;
        let mut peak = 0.0_f32;
        for frame in 0..frames {
            let mut sample = 0.0;
            for voice in &mut preview.voices {
                if !voice.active {
                    continue;
                }
                voice
                    .filter
                    .process(voice.string.process(false, &mut noise));
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
            sample = fclamp(sample, -PREVIEW_PEAK, PREVIEW_PEAK);
            for pointer in channels.iter_mut() {
                // SAFETY: the caller promises each channel is writable for
                // `frames` samples.
                let channel = unsafe { core::slice::from_raw_parts_mut(*pointer, frames) };
                channel[frame] = fclamp(channel[frame] + sample, -1.0, 1.0);
            }
            peak = peak.max(sample.abs());
        }
        preview.noise = noise;
        peak
    })
}

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
        BassError, PADS, Pluck, preview_enable, preview_mix, preview_note, render, render_with,
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
        let _serial = PREVIEW_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        preview_enable(true);
        preview_reset();
        preview_note(3, 1.0);
        let rate = 44_100;
        let mut peak_late = 0.0_f32;
        // Two seconds of 128-frame blocks; the voice is capped at 1.2 s.
        for block in 0..(2 * rate / 128) {
            let mut left = vec![0.0_f32; 128];
            let mut right = vec![0.0_f32; 128];
            let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
            // SAFETY: both channels hold 128 samples.
            let peak = unsafe { preview_mix(&mut channels, 128, rate) };
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
        let _serial = PREVIEW_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _: Pluck = render(44_100, 0.5, 0.3, 0.6).expect("renders");
        assert_eq!(PADS, 16);
        preview_enable(true);
        preview_note(0, 0.8);
        let mut left = vec![0.0_f32; 4_096];
        let mut right = vec![0.0_f32; 4_096];
        // SAFETY: both channel buffers hold `left.len()` samples.
        let peak = unsafe {
            preview_mix(
                &mut [left.as_mut_ptr(), right.as_mut_ptr()],
                left.len(),
                44_100,
            )
        };
        assert!(peak > 0.0, "the strike is audible: {peak}");
        assert!(left.iter().any(|v| *v != 0.0));
        assert_eq!(left, right, "both channels get the same voice mix");
        preview_enable(false);
        // SAFETY: both channel buffers still hold `left.len()` samples.
        let silent = unsafe {
            preview_mix(
                &mut [left.as_mut_ptr(), right.as_mut_ptr()],
                left.len(),
                44_100,
            )
        };
        assert!(silent.abs() < f32::EPSILON);
    }
}
