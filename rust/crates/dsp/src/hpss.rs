//! Harmonic/percussive separation of a whole sample, offline.
//!
//! The mask is the classic median-filtered spectrogram one: the harmonic part of
//! a bin is its median over time, the percussive part its median over frequency,
//! and the split weight is `p^6 / (h^6 + p^6)` so that the decision is sharp
//! around the cross-over instead of leaving both outputs sounding half-filtered.
//!
//! The window is [`FRAME`] long with a [`HOP`] of a quarter of it, so the
//! overlap-add is `sum(w^2) = 1` after the gain normalisation at the end.  Both
//! outputs are the same length as the input and neither is gated or normalised:
//! the caller decides what to do with them.

use core::f32::consts::TAU;

use crate::AnalysisError;
use crate::fft::{Complex, Fft};
use crate::num::{f32_to_u32, i64_to_usize, u32_to_usize, usize_to_i64};
use crate::sample::u32_to_f32;

/// Analysis window length in samples.
pub const FRAME: usize = 2048;
/// Hop between successive analysis windows in samples.
pub const HOP: usize = 512;
/// Number of non-redundant bins of a `FRAME`-point real transform.
const BINS: usize = FRAME / 2 + 1;
/// Percussive median scale that biases the split towards keeping transients.
const PERCUSSIVE_BIAS: f32 = 0.55;

/// Split interleaved f32 audio into `(harmonic, percussive)` buffers.
///
/// Both outputs are interleaved like the input and the same length as it.
///
/// # Errors
///
/// Returns [`AnalysisError`] when the buffer is empty, ragged, has a channel
/// count other than one or two, carries a sample rate outside 8 kHz to 192 kHz,
/// or contains a non-finite sample.
pub fn split(
    input: &[f32],
    channels: usize,
    rate: u32,
) -> Result<(Vec<f32>, Vec<f32>), AnalysisError> {
    crate::validate(input, channels, rate)?;
    if !(1..=2).contains(&channels) {
        return Err(AnalysisError::UnsupportedChannels);
    }

    let frames = input.len() / channels;
    let rate_f = u32_to_f32(rate);
    let window_count = 1 + (frames + FRAME) / HOP;
    let harmonic_width = odd_width(0.476 * rate_f / u32_to_f32(u32::try_from(HOP).unwrap_or(1)));
    let percussive_width =
        odd_width(366.0 * u32_to_f32(u32::try_from(FRAME).unwrap_or(1)) / rate_f);

    let fft = Fft::new(FRAME).map_err(|_| AnalysisError::UnsupportedRequest)?;
    let window = analysis_window();
    let weights = overlap_weights(&window, window_count, frames);

    let cells = window_count * BINS;
    let mut magnitude = vec![0.0_f32; cells];
    let mut spectrogram = vec![Complex::ZERO; cells];
    let mut harmonic_median = vec![0.0_f32; cells];
    let mut harmonic = vec![0.0_f32; input.len()];
    let mut percussive = vec![0.0_f32; input.len()];

    let mut analysis = ChannelAnalysis {
        fft: &fft,
        window: &window,
        frames,
        window_count,
        harmonic_width,
        percussive_width,
        magnitude: &mut magnitude,
        spectrogram: &mut spectrogram,
        harmonic_median: &mut harmonic_median,
    };
    for channel in 0..channels {
        analyse_channel(
            input,
            channels,
            channel,
            &mut analysis,
            &mut harmonic,
            &mut percussive,
        );
    }

    for (t, &weight) in weights.iter().enumerate() {
        let gain = weight.max(1e-9);
        for channel in 0..channels {
            let index = t * channels + channel;
            harmonic[index] /= gain;
            percussive[index] /= gain;
        }
    }
    Ok((harmonic, percussive))
}

/// Scratch shared by both channel passes.
struct ChannelAnalysis<'a> {
    fft: &'a Fft,
    window: &'a [f32],
    frames: usize,
    window_count: usize,
    harmonic_width: usize,
    percussive_width: usize,
    magnitude: &'a mut [f32],
    spectrogram: &'a mut [Complex],
    harmonic_median: &'a mut [f32],
}

/// The Hann analysis/synthesis window.
fn analysis_window() -> Vec<f32> {
    let denominator = u32_to_f32(u32::try_from(FRAME - 1).unwrap_or(1));
    (0..FRAME)
        .map(|j| {
            let phase = TAU * u32_to_f32(u32::try_from(j).unwrap_or(0)) / denominator;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

/// Overlap-add gain: the sum of the squared windows covering each frame.
fn overlap_weights(window: &[f32], window_count: usize, frames: usize) -> Vec<f32> {
    let mut weights = vec![0.0_f32; frames];
    for f in 0..window_count {
        // A window that starts before the first sample still covers later ones,
        // so the edge frames need a per-sample check rather than a skip.
        let base = usize_to_i64(f * HOP) - usize_to_i64(FRAME);
        for (j, &w) in window.iter().enumerate() {
            let t = base + usize_to_i64(j);
            if t >= 0 && i64_to_usize(t) < frames {
                weights[i64_to_usize(t)] += w * w;
            }
        }
    }
    weights
}

/// Analyse one channel and overlap-add both components into the outputs.
fn analyse_channel(
    input: &[f32],
    channels: usize,
    channel: usize,
    analysis: &mut ChannelAnalysis<'_>,
    harmonic: &mut [f32],
    percussive: &mut [f32],
) {
    let frame_count = analysis.window_count;
    let bins = BINS;
    let mut frame = vec![Complex::ZERO; FRAME];
    let mut other = vec![Complex::ZERO; FRAME];
    let mut percussive_median = vec![0.0_f32; BINS];
    let mut median = SlidingMedian::new(analysis.harmonic_width.max(analysis.percussive_width));

    for f in 0..frame_count {
        for (j, &w) in analysis.window.iter().enumerate() {
            let source = reflect_sample(f * HOP + j, FRAME, analysis.frames);
            frame[j] = Complex::from_real(input[source * channels + channel] * w);
        }
        analysis.fft.forward_transform(&mut frame);
        for (b, value) in frame.iter().take(bins).enumerate() {
            let cell = f * bins + b;
            analysis.spectrogram[cell] = *value;
            analysis.magnitude[cell] = value.magnitude();
        }
    }

    // Median over time, for every bin.
    {
        let magnitude = &*analysis.magnitude;
        let harmonic_median = &mut *analysis.harmonic_median;
        for b in 0..bins {
            median.run(
                frame_count,
                analysis.harmonic_width,
                |i| magnitude[i * bins + b],
                |i, value| harmonic_median[i * bins + b] = value,
            );
        }
    }

    // Median over frequency, for every frame, then mask and overlap-add.
    for f in 0..frame_count {
        {
            let magnitude = &*analysis.magnitude;
            median.run(
                bins,
                analysis.percussive_width,
                |b| magnitude[f * bins + b],
                |b, value| percussive_median[b] = value,
            );
        }
        for b in 0..bins {
            // Sixth power of each median: `(x^2)^3`, two multiplies more than a
            // `powf` and exactly the reference formula.
            let h = analysis.harmonic_median[f * bins + b];
            let p = PERCUSSIVE_BIAS * percussive_median[b];
            let (h2, p2) = (h * h, p * p);
            let (harmonic_power, percussive_power) = (h2 * h2 * h2, p2 * p2 * p2);
            let mask = percussive_power / (harmonic_power + percussive_power).max(1e-12);
            let z = analysis.spectrogram[f * bins + b];
            frame[b] = Complex::new(z.re * mask, z.im * mask);
            other[b] = Complex::new(z.re * (1.0 - mask), z.im * (1.0 - mask));
        }
        for b in bins..FRAME {
            frame[b] = Complex::new(frame[FRAME - b].re, -frame[FRAME - b].im);
            other[b] = Complex::new(other[FRAME - b].re, -other[FRAME - b].im);
        }
        analysis.fft.inverse_transform(&mut frame);
        analysis.fft.inverse_transform(&mut other);
        let base = usize_to_i64(f * HOP) - usize_to_i64(FRAME);
        for (j, &w) in analysis.window.iter().enumerate() {
            let t = base + usize_to_i64(j);
            if t < 0 || i64_to_usize(t) >= analysis.frames {
                continue;
            }
            let index = i64_to_usize(t) * channels + channel;
            // `frame` carries the percussive mask, `other` the harmonic one.
            percussive[index] += frame[j].re * w;
            harmonic[index] += other[j].re * w;
        }
    }
}

/// Mirror a sample index into `0..length` so the median window has no edge
/// effect, exactly like the reference implementation's reflected boundaries.
fn reflect_sample(index: usize, offset: usize, length: usize) -> usize {
    if length <= 1 {
        return 0;
    }
    let length_i = usize_to_i64(length);
    let period = 2 * (length_i - 1);
    let mut reflected = (usize_to_i64(index) - usize_to_i64(offset)) % period;
    if reflected < 0 {
        reflected += period;
    }
    if reflected >= length_i {
        reflected = period - reflected;
    }
    i64_to_usize(reflected)
}

/// Mirror an index in a `0..length` dimension onto itself, for the median
/// window's reflected edges.
fn reflect_index(index: i64, length: usize) -> usize {
    if length <= 1 {
        return 0;
    }
    let length_i = usize_to_i64(length);
    let period = 2 * (length_i - 1);
    let mut reflected = index % period;
    if reflected < 0 {
        reflected += period;
    }
    if reflected >= length_i {
        reflected = period - reflected;
    }
    i64_to_usize(reflected)
}

/// Round `value` to the nearest integer and force it odd, so the sliding window
/// always has a well-defined centre.
fn odd_width(value: f32) -> usize {
    let rounded = (value + 0.5).floor().clamp(1.0, 4096.0);
    let count = f32_to_u32(rounded) | 1;
    u32_to_usize(count)
}

/// A sorted sliding window, so a median filter costs one removal and one
/// insertion per step instead of a full sort.
struct SlidingMedian {
    sorted: Vec<f32>,
    capacity: usize,
}

impl SlidingMedian {
    fn new(capacity: usize) -> Self {
        Self {
            sorted: vec![0.0; capacity],
            capacity,
        }
    }

    /// Filter `length` values addressed by `at` (already reflected) and report
    /// each median through `emit`.
    fn run<F, G>(&mut self, length: usize, width: usize, at: F, mut emit: G)
    where
        F: Fn(usize) -> f32,
        G: FnMut(usize, f32),
    {
        debug_assert!(width <= self.capacity && width > 0);
        let radius = width / 2;
        for j in 0..width {
            let value = at(reflect_index(offset_index(j, radius), length));
            self.insert(j, value);
        }
        for i in 0..length {
            emit(i, self.sorted[width / 2]);
            if i + 1 == length {
                break;
            }
            let old = at(reflect_index(offset_index(i, radius), length));
            let mut j = 0;
            while j + 1 < width && self.sorted[j].to_bits() != old.to_bits() {
                j += 1;
            }
            self.sorted.copy_within(j + 1..width, j);
            let next = at(reflect_index(
                offset_index(i, radius) + usize_to_i64(width),
                length,
            ));
            self.insert(width - 1, next);
        }
    }

    fn insert(&mut self, mut count: usize, value: f32) {
        while count > 0 && self.sorted[count - 1] > value {
            self.sorted[count] = self.sorted[count - 1];
            count -= 1;
        }
        self.sorted[count] = value;
    }
}

/// `index - radius` without underflowing the intermediate arithmetic.
fn offset_index(index: usize, radius: usize) -> i64 {
    usize_to_i64(index) - usize_to_i64(radius)
}
