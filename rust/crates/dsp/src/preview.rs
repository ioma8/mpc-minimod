//! The bass and synth preview: one process singleton, two voice banks.
//!
//! The preview is driven by the audio writer hook, which exists exactly once in
//! the process.  Parameters and pending pad strikes are atomics, so the pad and
//! UI threads never touch a voice.  Only the audio thread reaches the voices
//! through [`with_preview`], and `mix_slices` is `unsafe` so that contract is
//! visible at every call site.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

use crate::bass::{BassBank, Kind, NOMINAL_RATE, PADS};
use crate::dsp::{WhiteNoise, fclamp};
use crate::synth::SynthBank;

/// The preview engine; only the audio writer thread touches it.
struct Preview {
    rate: u32,
    kind: Kind,
    params: [f32; 3],
    bass: BassBank,
    synth: SynthBank,
    noise: WhiteNoise,
    was_enabled: bool,
    control_phase: u8,
    epoch: u32,
}

impl Preview {
    fn new() -> Self {
        let params = Kind::Bass.defaults();
        Self {
            rate: NOMINAL_RATE,
            kind: Kind::Bass,
            params,
            bass: BassBank::new(NOMINAL_RATE),
            synth: SynthBank::new(Kind::Pads, NOMINAL_RATE, Kind::Pads.defaults()),
            noise: WhiteNoise::default(),
            was_enabled: false,
            control_phase: 0,
            epoch: 0,
        }
    }

    /// Clear active voices on the audio thread.
    fn stop(&mut self) {
        self.bass.stop();
        self.synth.stop();
    }
}

/// The preview state, only ever touched by the audio writer thread.
struct PreviewCell(core::cell::UnsafeCell<Option<Preview>>);

// SAFETY: the engine is only ever reached through `with_preview`, which runs on
// the audio writer thread; the pad and UI threads only use the atomics below.
unsafe impl Sync for PreviewCell {}

static PREVIEW: PreviewCell = PreviewCell(core::cell::UnsafeCell::new(None));
/// Generation of pending pad strikes and audio-thread resets.
static RESET_EPOCH: AtomicU32 = AtomicU32::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);
/// The selected synth character, as a [`Kind::ALL`] index.
static KIND: AtomicU32 = AtomicU32::new(0);
/// Semitones added to every preview pad.
static TRANSPOSE: AtomicI32 = AtomicI32::new(0);
/// Brightness, decay and cutoff, as f32 bits.
static PARAMS: [AtomicU32; 3] = [
    AtomicU32::new(0.52_f32.to_bits()),
    AtomicU32::new(0.25_f32.to_bits()),
    AtomicU32::new(0.56_f32.to_bits()),
];
/// Epoch and velocity bits per pad; zero velocity means no strike.
static PENDING: [AtomicU64; PADS] = [const { AtomicU64::new(0) }; PADS];

#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
/// Only the flag is written here: the voices belong to the audio thread, which
/// silences every voice on its next block once the flag is off.
pub fn enable(enabled: bool) {
    if !enabled {
        reset();
    }
    ENABLED.store(enabled, Ordering::Release);
}

/// Select the character; the next audio block rebuilds the bank.
pub fn set_kind(kind: Kind) {
    KIND.store(u32::try_from(kind.index()).unwrap_or(0), Ordering::Release);
    reset();
}

/// The selected character.
#[must_use]
pub fn kind() -> Kind {
    Kind::from_index(i32::try_from(KIND.load(Ordering::Acquire)).unwrap_or(0))
}

/// Shift every preview pad by this many semitones.
pub fn set_transpose(semitones: i32) {
    TRANSPOSE.store(semitones, Ordering::Release);
}

/// The preview transpose in semitones.
#[must_use]
pub fn transpose() -> i32 {
    TRANSPOSE.load(Ordering::Acquire)
}

/// Set the three knobs, each 0..1.
pub fn set_params(params: [f32; 3]) {
    for (slot, value) in PARAMS.iter().zip(params) {
        slot.store(fclamp(value, 0.0, 1.0).to_bits(), Ordering::Release);
    }
}

/// Strike one pad; the audio thread picks the velocity up.
pub fn note(pad: usize, velocity: f32) {
    if velocity <= 0.0 || !velocity.is_finite() {
        return;
    }
    if let Some(slot) = PENDING.get(pad) {
        let epoch = u64::from(RESET_EPOCH.load(Ordering::Acquire));
        let value = (epoch << 32) | u64::from(fclamp(velocity, 0.0, 1.0).to_bits());
        slot.store(value, Ordering::Release);
    }
}

/// Ask the audio thread to silence every voice and drop pending strikes before
/// its next block, so a page that just opened never plays a stale note.
pub fn reset() {
    RESET_EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// Mix the preview into up to two channel slices.
///
/// `frames` is clamped to the shortest slice, so a caller can never be read past
/// its buffer.
///
/// # Safety
///
/// The audio writer thread must be the only caller: the preview is a process
/// singleton with no lock, and this mixes into caller-owned buffers.
pub unsafe fn mix_slices(channels: &mut [&mut [f32]], frames: usize, rate: u32) -> f32 {
    // The preview is stereo: ignore any extra output channels.
    let count = channels.len().min(2);
    let channels = &mut channels[..count];
    let frames = frames.min(
        channels
            .iter()
            .map(|channel| channel.len())
            .min()
            .unwrap_or(0),
    );
    mix(channels, frames, rate)
}

/// Mix the preview into the device's output slices and report its peak.
fn mix(channels: &mut [&mut [f32]], frames: usize, rate: u32) -> f32 {
    let epoch = RESET_EPOCH.load(Ordering::Acquire);
    with_preview(|preview| {
        if preview.epoch != epoch {
            preview.stop();
            preview.epoch = epoch;
        }
    });
    if !ENABLED.load(Ordering::Acquire)
        || channels.is_empty()
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

    let kind = kind();
    let params = [
        f32::from_bits(PARAMS[0].load(Ordering::Acquire)),
        f32::from_bits(PARAMS[1].load(Ordering::Acquire)),
        f32::from_bits(PARAMS[2].load(Ordering::Acquire)),
    ];
    with_preview(|preview| {
        let switched = preview.rate != rate || preview.kind != kind;
        preview.rate = rate;
        preview.kind = kind;
        if switched {
            preview.control_phase = 0;
            preview.params = params;
            if kind == Kind::Bass {
                preview.bass = BassBank::new(rate);
            } else {
                preview.synth = SynthBank::new(kind, rate, params);
            }
        }
        preview.was_enabled = true;
        let transpose = TRANSPOSE.load(Ordering::Acquire);
        for (pad, pending) in PENDING.iter().enumerate() {
            let value = pending.load(Ordering::Acquire);
            let bits = value as u32;
            if bits == 0 {
                continue;
            }
            if (value >> 32) as u32 != epoch {
                // A strike from a future reset belongs to the next block.
                if RESET_EPOCH.load(Ordering::Acquire) == epoch {
                    let _ = pending.compare_exchange(value, 0, Ordering::AcqRel, Ordering::Acquire);
                }
                continue;
            }
            if pending
                .compare_exchange(value, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            let velocity = f32::from_bits(bits);
            match kind {
                Kind::Bass => preview
                    .bass
                    .trigger(pad, velocity, params[0], params[1], params[2], transpose),
                _ => preview.synth.trigger(pad, velocity, params, transpose),
            }
        }
        let mut peak = 0.0_f32;
        for frame in 0..frames {
            // Ramp controls over roughly 20 ms, updating filters every 32 frames.
            if preview.control_phase == 0 {
                let amount = 1.0 - (-(32.0 / (rate as f32 * 0.02))).exp();
                for (current, target) in preview.params.iter_mut().zip(params) {
                    *current += (target - *current) * amount;
                }
                if kind == Kind::Bass {
                    preview
                        .bass
                        .configure(preview.params[0], preview.params[1], preview.params[2]);
                } else {
                    preview.synth.configure(preview.params);
                }
            }
            preview.control_phase = (preview.control_phase + 1) % 32;
            let sample = match kind {
                Kind::Bass => preview.bass.frame(&mut preview.noise, rate),
                _ => preview.synth.frame(&mut preview.noise, rate),
            };
            for channel in channels.iter_mut() {
                channel[frame] = fclamp(channel[frame] + sample, -1.0, 1.0);
            }
            peak = peak.max(sample.abs());
        }
        peak
    })
}

#[cfg(test)]
mod transpose_tests {
    use super::{set_transpose, transpose};

    #[test]
    fn reset_discards_old_strikes_but_keeps_new_ones() {
        use super::{Kind, TEST_LOCK, enable, mix, note, reset, set_kind, set_params};
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        enable(false);
        set_kind(Kind::Pads);
        set_params(Kind::Pads.defaults());
        note(0, 1.0);
        reset();
        note(1, 1.0);
        enable(true);
        let mut audio = [0.0; 4096];
        let peak = mix(&mut [&mut audio], 4096, 44_100);
        assert_eq!(
            super::PENDING[0].load(super::Ordering::Acquire),
            0,
            "old strike survived reset"
        );
        enable(false);
        set_kind(Kind::Bass);
        set_params(Kind::Bass.defaults());
        assert!(peak > 1e-4, "new strike was lost after reset");
    }

    #[test]
    fn parameter_ramp_is_independent_of_block_size() {
        use super::{Kind, TEST_LOCK, enable, mix, set_kind, set_params, with_preview};
        fn run(chunks: &[usize]) -> [f32; 3] {
            set_kind(Kind::Bass);
            enable(true);
            set_params([0.0; 3]);
            let mut sample = [0.0; 1];
            mix(&mut [&mut sample], 1, 44_100);
            set_kind(Kind::Pads);
            mix(&mut [&mut sample], 1, 44_100);
            set_params([1.0; 3]);
            for &size in chunks {
                let mut buffer = vec![0.0; size];
                mix(&mut [&mut buffer], size, 44_100);
            }
            with_preview(|preview| preview.params)
        }
        let _serial = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let regular = run(&[128]);
        let irregular = run(&[1, 7, 13, 31, 3, 73]);
        enable(false);
        set_kind(Kind::Bass);
        set_params(Kind::Bass.defaults());
        assert!(
            regular
                .iter()
                .zip(irregular)
                .all(|(a, b)| (a - b).abs() < 1e-7)
        );
    }

    #[test]
    fn the_preview_transpose_round_trips() {
        set_transpose(8);
        assert_eq!(transpose(), 8);
        set_transpose(-8);
        assert_eq!(transpose(), -8);
        set_transpose(0);
    }
}
