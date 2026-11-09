//! `minimod` — the feature set built on [`mpc_sample_sdk`].
//!
//! The SDK owns the firmware image, the hooks, the input/audio/MIDI plumbing and
//! the app's objects.  This crate only contains the features: the SAMPLE OPTIONS
//! menu, `AutoTrim`, HPSS, Spectral Chop, the effects and bass synth, the 16 Levels
//! pitch shift, external MIDI chromatic playback, and the Live Looper.
//!
//! Nothing in here may panic: a panic in a preloaded library aborts the sampler.
//! Everything here is safe Rust: every firmware access and hook is an SDK call.

#![deny(clippy::panic, clippy::todo, clippy::unimplemented)]

pub mod actions;
pub mod autotrim;
pub mod backend;
pub mod buttons;
pub mod cc;
pub mod cfg;
pub mod input;
pub mod jobflow;
pub mod levels;
pub mod looper_view;
pub mod menu;
pub mod midi;
pub mod module;
pub mod overlay;
pub mod routing;
pub mod settings;
pub mod spectral;
pub mod spectral_machine;
pub mod ui;

pub use mpc_sample_sdk::{
    ButtonEvent, ButtonFlow, CcEvent, CcFlow, EncoderEvent, EncoderFlow, Mod, NoteEvent, NoteFlow,
    PadAction, PadEvent, PadFlow, PaintEvent, SurfaceEvent, SurfaceFlow,
};

// The SDK's modules, under the names the feature code already uses.
pub(crate) use mpc_sample_sdk::draw as paint;
pub(crate) use mpc_sample_sdk::platform::{
    clock, control, pads, page, paths, sample, selection, slice as chop, status, trim, wav,
};
