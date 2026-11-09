//! The HPSS split, in the shape the render job wants: safe slices in, safe
//! buffers out.

use minimod_dsp::hpss;

/// Split interleaved audio into harmonic and percussive parts.
///
/// Returns `None` when the geometry is unusable or the split fails, which the
/// job records as a failed render.
#[must_use]
pub fn split(input: &[f32], channels: usize, rate: u32) -> Option<(Vec<f32>, Vec<f32>)> {
    hpss::split(input, channels, rate).ok()
}
