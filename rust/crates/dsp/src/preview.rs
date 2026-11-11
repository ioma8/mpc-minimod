//! The bass and synth preview: one process singleton, two voice banks.
//!
//! The preview is driven by the audio writer hook, which exists exactly once in
//! the process.  Parameters and pending pad strikes are atomics, so the pad and
//! UI threads never touch a voice.  Only the audio thread reaches the voices
//! through [`with_preview`], and `mix_slices` is `unsafe` so that contract is
//! visible at every call site.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
        }
    }

    /// Clear every voice and every strike that has not been picked up yet.
    fn stop(&mut self) {
        self.bass.stop();
        self.synth.stop();
        for pending in &PENDING {
            pending.store(0, Ordering::Release);
        }
    }
}

/// The preview state, only ever touched by the audio writer thread.
struct PreviewCell(core::cell::UnsafeCell<Option<Preview>>);

// SAFETY: the engine is only ever reached through `with_preview`, which runs on
// the audio writer thread; the pad and UI threads only use the atomics below.
unsafe impl Sync for PreviewCell {}

static PREVIEW: PreviewCell = PreviewCell(core::cell::UnsafeCell::new(None));
/// A reset the audio thread has not run yet.
static RESET: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
/// The selected synth character, as a [`Kind::ALL`] index.
static KIND: AtomicU32 = AtomicU32::new(0);
/// Brightness, decay and cutoff, as f32 bits.
static PARAMS: [AtomicU32; 3] = [
    AtomicU32::new(0.52_f32.to_bits()),
    AtomicU32::new(0.25_f32.to_bits()),
    AtomicU32::new(0.56_f32.to_bits()),
];
/// Velocity per pad, as f32 bits; zero means "no strike pending".
static PENDING: [AtomicU32; PADS] = [const { AtomicU32::new(0) }; PADS];

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
        slot.store(fclamp(velocity, 0.0, 1.0).to_bits(), Ordering::Release);
    }
}

/// Ask the audio thread to silence every voice and drop pending strikes before
/// its next block, so a page that just opened never plays a stale note.
pub fn reset() {
    RESET.store(true, Ordering::Release);
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
    if RESET.swap(false, Ordering::AcqRel) {
        with_preview(Preview::stop);
    }
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
        let params_changed = preview
            .params
            .iter()
            .zip(params)
            .any(|(old, new)| old.to_bits() != new.to_bits());
        let switched = preview.rate != rate || preview.kind != kind;
        preview.rate = rate;
        preview.kind = kind;
        preview.params = params;
        if kind == Kind::Bass {
            if switched {
                preview.bass = BassBank::new(rate);
            } else {
                // The bass keeps its voices across a knob move.
                preview.bass.configure(params[0], params[2]);
            }
        } else if switched || params_changed {
            preview.synth = SynthBank::new(kind, rate, params);
        }
        preview.was_enabled = true;
        for (pad, pending) in PENDING.iter().enumerate() {
            let bits = pending.swap(0, Ordering::AcqRel);
            if bits == 0 {
                continue;
            }
            let velocity = f32::from_bits(bits);
            match kind {
                Kind::Bass => preview
                    .bass
                    .trigger(pad, velocity, params[0], params[1], params[2]),
                _ => preview.synth.trigger(pad, velocity, params),
            }
        }
        let mut peak = 0.0_f32;
        for frame in 0..frames {
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
