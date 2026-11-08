//! Spectral Chop onset detection.
//!
//! A slice is a region, not a note: it has to be a musical fragment that can be
//! recombined, so boundaries go where the music changes identity (a new chord, a
//! new phrase, a different drum) and always land on a real attack.
//!
//! * **candidates** — every frame carrying an attack: per-band log flux, a
//!   threshold of `0.1 x mean + 2 x local median`, `0.5 x mean` prominence above
//!   the valley since the last hit, at least 40 ms apart;
//! * **region** — the shortest slice follows the material's onset density,
//!   `0.45 / onsets-per-second` clamped to 0.18 .. 0.75 s;
//! * **keep** — a candidate is kept when the slice is long enough *and* the
//!   sound has moved on since the last cut: cosine distance of 12-bin chroma
//!   (chord change) or of 48 log bands (different sound);
//! * **position** — the steepest 1 ms step within +-15 ms, backed off to 15 % of
//!   the transient peak, so the cut sits on the attack;
//! * **limits** — no cuts in the last 120 ms, at most `max_positions` slices,
//!   first in time.

use core::f32::consts::TAU;

use crate::fft::{Complex, Fft};
use crate::num::{f32_to_u32, f32_to_usize};
use crate::sample::u32_to_f32;
use crate::{AnalysisError, validate};

/// Analysis window length in samples.
pub const FRAME: usize = 1024;
/// Hop between successive analysis frames in samples.
pub const HOP: usize = 512;
/// Upper bound on analysed frames; longer samples are truncated.
const MAX_FRAMES: usize = 65_536;
/// Number of non-redundant bins of a `FRAME`-point real transform.
const BINS: usize = FRAME / 2 + 1;
/// Chroma bins (pitch classes).
const CHROMA: usize = 12;
/// Log-spaced timbre bands.
const TBANDS: usize = 48;
/// Shortest gap between two slice boundaries, in seconds.
const MINIMUM_GAP: f32 = 0.040;
/// No cuts this close to the end of the sample, in seconds.
const END_GUARD: f32 = 0.120;
/// Lead-in before the first detected attack that becomes its own slice.
const LEAD_IN: f32 = 0.050;

/// Detected slice boundaries and how many candidate onsets were considered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Onsets {
    /// Slice boundaries in sample frames, ascending, starting at zero when the
    /// first attack arrives later than 50 ms.
    pub positions: Vec<u32>,
    /// Number of candidate onsets found before the region and novelty filters.
    pub detected: u32,
}

/// Detect slice boundaries in an interleaved f32 buffer.
///
/// # Errors
///
/// Returns [`AnalysisError`] when the buffer is empty, ragged, has no channels,
/// carries an unsupported sample rate, holds a non-finite sample, or when
/// `max_positions` is zero or the sample is shorter than half an analysis frame.
pub fn detect(
    input: &[f32],
    channels: usize,
    rate: u32,
    max_positions: usize,
) -> Result<Onsets, AnalysisError> {
    validate(input, channels, rate)?;
    let frames = input.len() / channels;
    if max_positions == 0 || frames < FRAME / 2 {
        return Err(AnalysisError::UnsupportedRequest);
    }

    let mono = mono_mix(input, channels);
    let window_count = (frames + HOP - 1).div_ceil(HOP).min(MAX_FRAMES);
    let rate_f = u32_to_f32(rate);
    let fft = Fft::new(FRAME).map_err(|_| AnalysisError::UnsupportedRequest)?;

    let window = analysis_window();
    let (magnitude, mut chroma, mut timbre) =
        analyse_frames(&fft, &mono, frames, window_count, rate_f, &window);
    normalise_frames(&mut chroma, &mut timbre, window_count);

    let flux = attack_novelty(&magnitude, window_count, rate_f);
    let hits = novelty_peaks(&flux, window_count, rate_f);
    let kept = select_cuts(
        &chroma,
        &timbre,
        &hits,
        frames,
        window_count,
        rate_f,
        max_positions,
    );
    let positions = refine_positions(&mono, &kept, rate, frames, max_positions);

    Ok(Onsets {
        positions,
        detected: u32::try_from(kept.len()).unwrap_or(0),
    })
}

/// The Hann analysis window.
fn analysis_window() -> Vec<f32> {
    let denominator = u32_to_f32(u32::try_from(FRAME - 1).unwrap_or(1));
    (0..FRAME)
        .map(|j| {
            let phase = TAU * u32_to_f32(u32::try_from(j).unwrap_or(0)) / denominator;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

/// Average the interleaved channels into one mono buffer.
fn mono_mix(input: &[f32], channels: usize) -> Vec<f32> {
    let frames = input.len() / channels;
    let divisor = u32_to_f32(u32::try_from(channels).unwrap_or(1));
    (0..frames)
        .map(|t| {
            let mut sum = 0.0;
            for channel in 0..channels {
                sum += input[t * channels + channel];
            }
            sum / divisor
        })
        .collect()
}

/// Magnitude spectrogram plus its chroma and timbre projections.
fn analyse_frames(
    fft: &Fft,
    mono: &[f32],
    frames: usize,
    window_count: usize,
    rate: f32,
    window: &[f32],
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut magnitude = vec![0.0_f32; window_count * BINS];
    let mut chroma = vec![0.0_f32; window_count * CHROMA];
    let mut timbre = vec![0.0_f32; window_count * TBANDS];
    let mut frame = vec![Complex::ZERO; FRAME];
    let edges = timbre_edges(rate);
    let (in_chroma_range, pitch_class) = bin_pitch_classes(rate);

    for f in 0..window_count {
        let start = f * HOP;
        for j in 0..FRAME {
            let t = start + j;
            let sample = if t < frames { mono[t] * window[j] } else { 0.0 };
            frame[j] = Complex::from_real(sample);
        }
        fft.forward_transform(&mut frame);
        let row = &mut magnitude[f * BINS..][..BINS];
        for (b, slot) in row.iter_mut().enumerate() {
            *slot = frame[b].magnitude();
            if in_chroma_range[b] {
                chroma[f * CHROMA + pitch_class[b]] += *slot;
            }
        }
        for (band, window_edges) in edges.windows(2).enumerate() {
            let sum: f32 = row[window_edges[0]..window_edges[1]].iter().sum();
            timbre[f * TBANDS + band] = sum;
        }
    }
    (magnitude, chroma, timbre)
}

/// Band edges in bins for the 48 log-spaced timbre bands, 60 Hz to 12 kHz.
fn timbre_edges(rate: f32) -> Vec<usize> {
    let low = 60.0;
    let high = (rate / 2.0).min(12_000.0);
    let ratio = (high / low).ln() / u32_to_f32(u32::try_from(TBANDS).unwrap_or(1));
    let frame = u32_to_f32(u32::try_from(FRAME).unwrap_or(1));
    (0..=TBANDS)
        .map(|i| {
            let frequency = low * (ratio * u32_to_f32(u32::try_from(i).unwrap_or(0))).exp();
            f32_to_usize(frequency * frame / rate).min(BINS - 1)
        })
        .collect()
}

/// Whether a bin belongs to the chroma range and which pitch class it is.
fn bin_pitch_classes(rate: f32) -> ([bool; BINS], [usize; BINS]) {
    let mut in_range = [false; BINS];
    let mut pitch = [0_usize; BINS];
    let frame = u32_to_f32(u32::try_from(FRAME).unwrap_or(1));
    for (b, (in_range, class)) in in_range.iter_mut().zip(pitch.iter_mut()).enumerate() {
        let frequency = u32_to_f32(u32::try_from(b).unwrap_or(0)) * rate / frame;
        if (55.0..=2_000.0).contains(&frequency) {
            *in_range = true;
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
            // The reference rounds to the nearest semitone (`lroundf`); a
            // truncating cast would put half the bins a semitone off and change
            // every chroma distance that the cut filter compares.
            let rounded = f32_to_u32(midi.round());
            *class = usize::try_from(rounded % u32::try_from(CHROMA).unwrap_or(1)).unwrap_or(0);
        }
    }
    (in_range, pitch)
}

/// Log-normalise every frame of the chroma and timbre matrices.
fn normalise_frames(chroma: &mut [f32], timbre: &mut [f32], window_count: usize) {
    for f in 0..window_count {
        let chroma_energy = {
            let row = &mut chroma[f * CHROMA..][..CHROMA];
            for value in row.iter_mut() {
                *value = value.ln_1p();
            }
            row.iter().map(|value| value * value).sum::<f32>()
        };
        let timbre_energy = {
            let row = &mut timbre[f * TBANDS..][..TBANDS];
            for value in row.iter_mut() {
                *value = value.ln_1p();
            }
            row.iter().map(|value| value * value).sum::<f32>()
        };
        normalise_row(&mut chroma[f * CHROMA..][..CHROMA], chroma_energy.sqrt());
        normalise_row(&mut timbre[f * TBANDS..][..TBANDS], timbre_energy.sqrt());
    }
}

/// Divide one row by its norm when the norm is meaningful.
fn normalise_row(row: &mut [f32], norm: f32) {
    if norm > 1e-9 {
        for value in row.iter_mut() {
            *value /= norm;
        }
    }
}

/// Per-band log flux, bands split at 250 Hz and 2 kHz, each normalised by its
/// own mean so no band dominates the novelty curve.
fn attack_novelty(magnitude: &[f32], window_count: usize, rate: f32) -> Vec<f32> {
    let frame = u32_to_f32(u32::try_from(FRAME).unwrap_or(1));
    let first = f32_to_usize(250.0 * frame / rate).clamp(1, BINS - 2);
    let second = f32_to_usize(2_000.0 * frame / rate)
        .max(first + 1)
        .min(BINS - 1);
    let edges = [0, first, second, BINS];
    let mut flux = vec![0.0_f32; window_count];
    for pair in edges.windows(2) {
        let (low, high) = (pair[0], pair[1]);
        let mut values = vec![0.0_f32; window_count];
        let mut total = 0.0_f64;
        for (f, value) in values.iter_mut().enumerate() {
            let mut sum = 0.0;
            if f > 0 {
                for b in low..high {
                    let current = magnitude[f * BINS + b].ln_1p();
                    let previous = magnitude[(f - 1) * BINS + b].ln_1p();
                    let difference = current - previous;
                    if difference > 0.0 {
                        sum += difference;
                    }
                }
            }
            *value = sum;
            total += f64::from(sum);
        }
        let mean = if window_count > 0 {
            (total / window_count as f64) as f32
        } else {
            0.0
        };
        let mean = if mean <= 1e-9 { 1e-9 } else { mean };
        for (slot, value) in flux.iter_mut().zip(values.iter()) {
            *slot += value / mean;
        }
    }
    flux
}

/// Candidate onsets: peaks of the novelty curve that clear the adaptive
/// threshold and the prominence requirement.
fn novelty_peaks(flux: &[f32], window_count: usize, rate: f32) -> Vec<usize> {
    let total: f64 = flux.iter().map(|value| f64::from(*value)).sum();
    let mean = if window_count > 0 {
        (total / window_count as f64) as f32
    } else {
        0.0
    };
    let threshold_floor = 0.10 * mean;
    let prominence = 0.50 * mean;

    let hop = u32_to_f32(u32::try_from(HOP).unwrap_or(1));
    let radius = f32_to_usize(0.5 * rate / hop + 0.5).max(1);
    let gap = i64::try_from(f32_to_usize((MINIMUM_GAP * rate / hop).ceil()).max(1)).unwrap_or(1);

    let mut hits = Vec::new();
    let mut last: i64 = -1_000_000;
    let mut valley = f32::MAX;
    let mut scratch: Vec<f32> = Vec::new();
    for f in 0..window_count {
        let value = flux[f];
        let left = if f > 0 { flux[f - 1] } else { f32::MIN };
        let right = if f + 1 < window_count {
            flux[f + 1]
        } else {
            f32::MIN
        };
        let low = f.saturating_sub(radius);
        let high = (f + radius).min(window_count - 1);
        let median = median(&mut scratch, &flux[low..=high]);
        let far_enough = i64::try_from(f).unwrap_or(i64::MAX) - last >= gap;
        let peaked = value > left && value > right;
        if peaked
            && value >= threshold_floor + 2.0 * median
            && f + 2 < window_count
            && far_enough
            && (hits.is_empty() || value - valley >= prominence)
        {
            hits.push(f);
            last = i64::try_from(f).unwrap_or(i64::MAX);
            valley = f32::MAX;
        } else if value < valley {
            valley = value;
        }
    }
    hits
}

/// Keep the candidates that are far enough apart and whose material changed.
fn select_cuts(
    chroma: &[f32],
    timbre: &[f32],
    hits: &[usize],
    frames: usize,
    window_count: usize,
    rate: f32,
    max_positions: usize,
) -> Vec<usize> {
    let hop = u32_to_f32(u32::try_from(HOP).unwrap_or(1));
    let duration = u32_to_f32(u32::try_from(frames).unwrap_or(u32::MAX)) / rate;
    let density = hits.len() as f32 / duration.max(0.2);
    let region_seconds = (0.45 / density.max(0.4)).clamp(0.18, 0.75);
    let minimum_gap = i64::try_from(f32_to_usize(region_seconds * rate / hop)).unwrap_or(0);
    let window = f32_to_usize(0.2 * rate / hop).max(2);
    let end_guard = i64::try_from(f32_to_usize(END_GUARD * rate / hop)).unwrap_or(0);

    let mut spectra = Spectra {
        chroma,
        timbre,
        window_count,
        window,
        chroma_mean: [0.0; CHROMA],
        timbre_mean: [0.0; TBANDS],
    };
    let mut kept: Vec<usize> = Vec::new();
    for &f in hits {
        if kept.len() >= max_positions {
            break;
        }
        let position = i64::try_from(f).unwrap_or(i64::MAX);
        if i64::try_from(window_count).unwrap_or(i64::MAX) - position < end_guard {
            continue;
        }
        if let Some(&previous) = kept.last() {
            let previous_i = i64::try_from(previous).unwrap_or(i64::MAX);
            if position - previous_i < minimum_gap {
                continue;
            }
            if !spectra.changed(previous, f) {
                continue;
            }
        }
        kept.push(f);
    }
    kept
}

/// The chroma and timbre matrices together with the scratch means the novelty
/// comparison needs.
struct Spectra<'a> {
    chroma: &'a [f32],
    timbre: &'a [f32],
    window_count: usize,
    window: usize,
    chroma_mean: [f32; CHROMA],
    timbre_mean: [f32; TBANDS],
}

impl Spectra<'_> {
    /// Whether the material changed enough between two candidate positions to
    /// be worth a cut: chroma (chord) or timbre (sound) cosine distance.
    fn changed(&mut self, previous: usize, current: usize) -> bool {
        let low = previous.saturating_sub(self.window);
        self.fill_means(low, previous - low);
        let (before_chroma, before_timbre) = self.norms();

        let high = (current + self.window).min(self.window_count);
        let following = high - current;
        let mut chroma_dot = 0.0;
        let mut timbre_dot = 0.0;
        for k in current..high {
            for b in 0..CHROMA {
                chroma_dot += self.chroma[k * CHROMA + b] * self.chroma_mean[b];
            }
            for b in 0..TBANDS {
                timbre_dot += self.timbre[k * TBANDS + b] * self.timbre_mean[b];
            }
        }
        if following > 0 {
            chroma_dot /= following as f32;
            timbre_dot /= following as f32;
        }

        self.fill_means(current, following);
        let (after_chroma, after_timbre) = self.norms();

        // Silence on one side is a change, not a match.
        let chroma_distance = if before_chroma > 1e-9 && after_chroma > 1e-9 {
            1.0 - chroma_dot / (before_chroma.sqrt() * after_chroma.sqrt())
        } else {
            1.0
        };
        let timbre_distance = if before_timbre > 1e-9 && after_timbre > 1e-9 {
            1.0 - timbre_dot / (before_timbre.sqrt() * after_timbre.sqrt())
        } else {
            1.0
        };
        !(chroma_distance < 0.05 && timbre_distance < 0.15)
    }

    /// Average the `count` frames starting at `start` into the mean buffers.
    fn fill_means(&mut self, start: usize, count: usize) {
        self.chroma_mean.fill(0.0);
        self.timbre_mean.fill(0.0);
        let high = start.saturating_add(count).min(self.window_count);
        for k in start..high {
            for b in 0..CHROMA {
                self.chroma_mean[b] += self.chroma[k * CHROMA + b];
            }
            for b in 0..TBANDS {
                self.timbre_mean[b] += self.timbre[k * TBANDS + b];
            }
        }
        if count > 0 {
            let divisor = count as f32;
            for value in &mut self.chroma_mean {
                *value /= divisor;
            }
            for value in &mut self.timbre_mean {
                *value /= divisor;
            }
        }
    }

    fn norms(&self) -> (f32, f32) {
        let chroma = self.chroma_mean.iter().map(|value| value * value).sum();
        let timbre = self.timbre_mean.iter().map(|value| value * value).sum();
        (chroma, timbre)
    }
}

/// Refine each cut to the steepest step of the attack it landed on.
fn refine_positions(
    mono: &[f32],
    kept: &[usize],
    rate: u32,
    frames: usize,
    max_positions: usize,
) -> Vec<u32> {
    let mut positions: Vec<u32> = Vec::new();
    let step = f32_to_usize(0.001 * u32_to_f32(rate)).max(1);
    let span = f32_to_usize(0.015 * u32_to_f32(rate));
    let minimum_step = f32_to_u32(MINIMUM_GAP * u32_to_f32(rate));
    for &f in kept {
        if positions.len() >= max_positions {
            break;
        }
        let pos = f * HOP;
        let start = pos.saturating_sub(span);
        let end = (pos + span).min(frames);
        let cells = (end - start) / step;
        if cells < 4 {
            positions.push(u32::try_from(pos).unwrap_or(u32::MAX));
            continue;
        }
        let mut best = 0.0_f32;
        let mut best_index = 0;
        for k in 0..cells {
            let level = envelope_max(mono, start + k * step, step);
            if level > best {
                best = level;
                best_index = k;
            }
        }
        let mut onset = best_index;
        let level = 0.15 * best;
        while onset > 0 && envelope_max(mono, start + (onset - 1) * step, step) > level {
            onset -= 1;
        }
        let attack = u32::try_from(start + onset * step).unwrap_or(u32::MAX);
        // The previous boundary is at least 40 ms back and refinement moves at
        // most 15 ms, so the subtraction cannot wrap; `saturating_sub` keeps it
        // total anyway.
        if positions
            .last()
            .is_some_and(|&last| attack.saturating_sub(last) < minimum_step)
        {
            continue;
        }
        positions.push(attack);
    }

    if let Some(&first) = positions.first()
        && first > f32_to_u32(LEAD_IN * u32_to_f32(rate))
        && positions.len() < max_positions
    {
        positions.insert(0, 0);
    }
    positions
}

/// Median of a slice, sorting a caller-owned scratch buffer.
fn median(scratch: &mut Vec<f32>, values: &[f32]) -> f32 {
    scratch.clear();
    scratch.extend_from_slice(values);
    scratch.sort_unstable_by(f32::total_cmp);
    scratch[scratch.len() / 2]
}

/// Peak magnitude of one hop, used to find the steepest step of an attack.
fn envelope_max(mono: &[f32], start: usize, hop: usize) -> f32 {
    mono[start..start + hop]
        .iter()
        .fold(0.0_f32, |peak, value| peak.max(value.abs()))
}
