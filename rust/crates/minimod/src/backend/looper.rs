//! The live looper engine's safe entry points for the feature code.

use minimod_dsp::looper::{Engine, LatencySource};
pub use minimod_dsp::looper::{MAX_FRAMES, MESSAGE_LEN, PADS, PadView, RATE, State, View};

/// Convert a 0..1 level to the app's meter scale.
#[must_use]
pub fn level(value: f32) -> u32 {
    minimod_dsp::num::f32_to_u32(value.clamp(0.0, 1.0) * 290.0).min(290)
}

/// Open the fullscreen view.  Returns non-zero on success.
#[must_use]
pub fn looper_activate() -> i32 {
    i32::from(Engine::open())
}

/// Whether the view is open.
#[must_use]
pub fn looper_is_open() -> i32 {
    i32::from(Engine::is_open())
}

/// A pad press.  Returns non-zero when the looper owns the event.
#[must_use]
pub fn looper_pad(pad: i32, shift: i32, erase: i32) -> i32 {
    let index = usize::try_from(pad).unwrap_or(usize::MAX);
    i32::from(Engine::pad(index, shift != 0, erase != 0))
}

/// A control-surface button while the view is open.
#[must_use]
pub fn looper_button(code: u32, down: i32, shift: i32, erase: i32) -> i32 {
    i32::from(Engine::button(code, down != 0, shift != 0, erase != 0))
}

/// Set MAIN VOLUME, 0..127.
pub fn looper_set_master(value: u32) {
    Engine::set_master(value);
}

/// The current MAIN VOLUME.
#[must_use]
pub fn looper_master() -> u32 {
    Engine::master()
}

/// Set the record latency in frames and where it came from.
pub fn looper_set_latency_ex(frames: u32, source: i8) {
    let code = u8::try_from(source).unwrap_or(b'E');
    Engine::set_latency(frames, LatencySource::from_code(code));
}

/// Frames captured but not yet consumed by playback.
#[must_use]
pub fn looper_input_backlog() -> u32 {
    Engine::input_backlog()
}

/// Show a short status line.
pub fn looper_notice(text: &str) {
    Engine::notice(text);
}

/// Feed the first planar capture channel.
pub fn looper_input_s32(channels: &[&[i32]]) {
    if let Some(first) = channels.first() {
        Engine::input_s32(first);
    }
}

/// Feed interleaved S32 capture frames.
pub fn looper_input_s32_interleaved(frames: &[i32], channels: usize) {
    if channels != 0 {
        Engine::input_s32_interleaved(frames, channels);
    }
}

/// Mix the loops into the interleaved S32 block that is about to be played.
pub fn looper_render_s32(frames: &mut [i32], channels: usize) {
    if channels != 0 {
        Engine::render_s32(frames, channels);
    }
}

/// Snapshot the engine for the paint hook.
pub fn looper_snapshot(view: &mut View) -> i32 {
    i32::from(Engine::snapshot(view))
}
