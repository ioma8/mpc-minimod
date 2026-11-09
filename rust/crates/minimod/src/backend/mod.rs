//! The only feature code that touches `minimod-dsp`.
//!
//! `actions` decides *what* to render; these modules know *how* the DSP crate
//! represents HPSS, the bass synth, the live looper and the preview mix.  Keeping
//! them together makes the rest of `minimod` free of DSP buffer conventions.

pub mod audio;
pub mod bass;
pub mod hpss;
pub mod looper;
pub mod spectral;
