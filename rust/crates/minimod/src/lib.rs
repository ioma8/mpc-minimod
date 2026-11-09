//! `minimod` — the `LD_PRELOAD` library that adds features to the stock app.
//!
//! This crate owns everything that has to be process-wide: the C ABI the
//! firmware mod exposes, the hook engine that patches the app's code sites, and
//! the feature glue that reads the app's own objects.  The audio DSP itself
//! lives in [`minimod_dsp`] and is host-testable on its own.
//!
//! Nothing in here may panic: a panic in a preloaded library aborts the sampler.
//! Every entry point validates its inputs, and the only `unwrap`s allowed are on
//! conversions from values this crate itself produced.

#![deny(clippy::panic, clippy::todo, clippy::unimplemented)]

pub mod analysis_ffi;
pub mod audio;
pub mod autotrim;
pub mod bass_ffi;
pub mod buttons;
pub mod cc;
pub mod cfg;
pub mod cfg_ffi;
pub mod chop;
pub mod clock;
pub mod effect;
pub mod effect_ffi;
pub mod firmware;
pub mod hooks;
pub mod input;
pub mod job;
pub mod jobflow;
pub mod levels;
pub mod looper_ffi;
pub mod looper_view;
pub mod menu;
pub mod midi;
pub mod overlay;
pub mod pads;
pub mod page;
pub mod paint;
pub mod paths;
pub mod paths_ffi;
pub mod props;
pub mod routing;
pub mod sample;
pub mod selection;
pub mod spectral;
pub mod spectral_machine;
pub mod status;
pub mod status_ffi;
pub mod substitute;
pub mod trim;
pub mod wav;
pub mod wav_ffi;
