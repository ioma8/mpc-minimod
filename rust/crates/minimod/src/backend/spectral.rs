//! Spectral onset detection, in the shape the chop state machine wants.

use minimod_dsp::spectral;

/// Slice boundaries for the selected audio.
///
/// Returns `None` when the buffer or request is unsupported.
#[must_use]
pub fn detect(input: &[f32], channels: usize, rate: u32, max_positions: usize) -> Option<Vec<u32>> {
    spectral::detect(input, channels, rate, max_positions)
        .ok()
        .map(|onsets| onsets.positions)
}
