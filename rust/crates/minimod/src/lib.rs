//! `minimod` — the feature set built on [`mpc_sample_sdk`].
//!
//! The SDK owns the firmware image, the hooks, the input/audio/MIDI plumbing and
//! the app's objects.  This crate only contains the features: the SAMPLE OPTIONS
//! menu, `AutoTrim`, HPSS, Spectral Chop, the effects and bass synth, the 16 Levels
//! pitch shift, external MIDI chromatic playback, and the Live Looper.
//!
//! Nothing in here may panic: a panic in a preloaded library aborts the sampler.
//! The feature code is safe except for small, documented `unsafe` blocks at the
//! SDK's explicit firmware boundaries.

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
pub mod looper_bounce;
pub mod looper_lights;
pub mod looper_view;
pub mod menu;
pub mod midi;
pub mod module;
pub mod overlay;
pub mod routing;
pub mod settings;
pub mod spectral;
pub mod spectral_machine;
pub mod synth_lights;
pub mod ui;

pub use mpc_sample_sdk::{
    ButtonEvent, ButtonFlow, CcEvent, CcFlow, EncoderEvent, EncoderFlow, Mod, NoteEvent, NoteFlow,
    PadAction, PadEvent, PadFlow, PaintEvent, SurfaceEvent, SurfaceFlow,
};

/// The SDK's draw surface with the firmware calls wrapped for the mod.
///
/// The wrappers keep the single safety argument in one place: every drawing
/// call is made from `on_paint` with the app's live `Graphics` handle on the UI
/// thread.
pub(crate) mod paint {
    use mpc_sample_sdk::draw::{
        colour as sdk_colour, rect as sdk_rect, start_repaint_pump as sdk_start_repaint_pump,
        text as sdk_text,
    };
    pub(crate) use mpc_sample_sdk::draw::{screen, show_toast, toast_text, toast_visible};

    /// Set the fill colour.
    pub(crate) fn colour(graphics: usize, value: u32) {
        // SAFETY: called only from `on_paint` with the live app Graphics.
        unsafe { sdk_colour(graphics, value) }
    }

    /// Fill a rectangle in the current colour.
    pub(crate) fn rect(graphics: usize, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: called only from `on_paint` with the live app Graphics.
        unsafe { sdk_rect(graphics, x, y, width, height) }
    }

    /// A stack buffer for one formatted line.
    ///
    /// The menu and the looper repaint several lines per frame; formatting them
    /// into `format!` strings would allocate on every frame for no reason.
    struct Line {
        bytes: [u8; 64],
        len: usize,
    }

    impl Line {
        const fn new() -> Self {
            Self {
                bytes: [0; 64],
                len: 0,
            }
        }

        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
        }
    }

    impl core::fmt::Write for Line {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let room = self.bytes.len() - self.len;
            let mut take = text.len().min(room);
            while !text.is_char_boundary(take) {
                take -= 1;
            }
            self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
            self.len += take;
            Ok(())
        }
    }

    /// Draw one line of formatted text without allocating.
    pub(crate) fn text_fmt(
        graphics: usize,
        args: core::fmt::Arguments<'_>,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        size: f32,
    ) {
        let mut line = Line::new();
        let _ = core::fmt::write(&mut line, args);
        text(graphics, line.as_str(), x, y, width, height, size);
    }

    /// Draw one line of text in the current colour.
    pub(crate) fn text(
        graphics: usize,
        line: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        size: f32,
    ) {
        // SAFETY: called only from `on_paint` with the live app Graphics.
        unsafe { sdk_text(graphics, line, x, y, width, height, size) }
    }

    /// Start the repaint pump used by the looper view.
    pub(crate) fn start_repaint_pump(component: fn() -> usize) {
        // SAFETY: `component` returns the paint hook's recorded component, or 0.
        unsafe { sdk_start_repaint_pump(component) }
    }
}

// The SDK's modules, under the names the feature code already uses.
pub(crate) use mpc_sample_sdk::platform::{
    clock, control, pads, page, paths, sample, selection, slice as chop, status, trim, wav,
};
