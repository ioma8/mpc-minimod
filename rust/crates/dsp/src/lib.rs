//! Audio DSP for the minimod firmware mod.
//!
//! Every module here is pure computation over slices: no firmware addresses, no
//! globals, no FFI.  That keeps the DSP testable on the host and lets the
//! `minimod` crate own all of the process-wide state and the C ABI.
//!
//! The two rules the audio paths live by:
//!
//! * nothing allocates or locks on an audio thread, so the realtime engines
//!   ([`looper`]) own preallocated buffers and publish state through atomics;
//! * sample conversion between the device's S32 frames and the internal f32
//!   audio goes through [`sample`], which documents the exact rounding.

use core::fmt;

pub mod fft;
pub mod hpss;
pub mod looper;
pub mod num;
pub mod sample;
pub mod spectral;
pub mod time;

#[cfg(test)]
mod golden_tests;

/// Why an offline analysis refused a buffer.
///
/// Both analyses run on a worker thread against a buffer the UI thread handed
/// over, so they validate it completely before touching any state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisError {
    /// No audio was supplied.
    Empty,
    /// The interleaved buffer does not divide evenly into channels.
    RaggedBuffer,
    /// Only mono and stereo are supported.
    UnsupportedChannels,
    /// The sample rate is outside the supported 8 kHz to 192 kHz range.
    UnsupportedRate,
    /// The buffer holds a NaN or an infinity.
    NotFinite,
    /// The caller asked for an output that cannot be produced.
    UnsupportedRequest,
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("no audio to analyse"),
            Self::RaggedBuffer => f.write_str("buffer length is not a whole number of frames"),
            Self::UnsupportedChannels => f.write_str("only mono and stereo are supported"),
            Self::UnsupportedRate => f.write_str("unsupported sample rate"),
            Self::NotFinite => f.write_str("audio contains a non-finite sample"),
            Self::UnsupportedRequest => f.write_str("unsupported analysis request"),
        }
    }
}

impl std::error::Error for AnalysisError {}

/// Validate a buffer both offline analyses share: non-empty, a supported sample
/// rate, a whole number of frames per channel, and finite samples.
///
/// # Errors
///
/// Returns the matching [`AnalysisError`] for the first check that fails.
pub fn validate(input: &[f32], channels: usize, rate: u32) -> Result<(), AnalysisError> {
    if input.is_empty() {
        return Err(AnalysisError::Empty);
    }
    if !(8_000..=192_000).contains(&rate) {
        return Err(AnalysisError::UnsupportedRate);
    }
    if channels == 0 || !input.len().is_multiple_of(channels) {
        return Err(if channels == 0 {
            AnalysisError::UnsupportedChannels
        } else {
            AnalysisError::RaggedBuffer
        });
    }
    if input.iter().any(|value| !value.is_finite()) {
        return Err(AnalysisError::NotFinite);
    }
    Ok(())
}
