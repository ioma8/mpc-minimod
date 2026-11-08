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

pub mod effect;
pub mod effect_ffi;
pub mod firmware;
pub mod hooks;
pub mod looper_ffi;
