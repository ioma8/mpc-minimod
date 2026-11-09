//! Entry points for the live looper engine, with the C calling convention the hook tables and callbacks use.
//!
//! These are the exact symbols and signatures the firmware glue calls, kept
//! identical to the C header they replaced so the paint and input hooks did not
//! have to change when the engine moved to Rust.

use core::ptr;

use minimod_dsp::looper::{Engine, LatencySource, View};

/// Open the fullscreen view.  Returns non-zero on success.
#[must_use]
pub extern "C" fn looper_activate() -> i32 {
    i32::from(Engine::open())
}

/// Whether the view is open.
#[must_use]
pub extern "C" fn looper_is_open() -> i32 {
    i32::from(Engine::is_open())
}

/// A pad press.  Returns non-zero when the looper owns the event.
#[must_use]
pub extern "C" fn looper_pad(pad: i32, shift: i32, erase: i32) -> i32 {
    let index = usize::try_from(pad).unwrap_or(usize::MAX);
    i32::from(Engine::pad(index, shift != 0, erase != 0))
}

/// A control-surface button while the view is open.
#[must_use]
pub extern "C" fn looper_button(code: u32, down: i32, shift: i32, erase: i32) -> i32 {
    i32::from(Engine::button(code, down != 0, shift != 0, erase != 0))
}

/// Set MAIN VOLUME, 0..127.
pub extern "C" fn looper_set_master(value: u32) {
    Engine::set_master(value);
}

/// The current MAIN VOLUME.
#[must_use]
pub extern "C" fn looper_master() -> u32 {
    Engine::master()
}

/// Set the record latency in frames and where it came from.
pub extern "C" fn looper_set_latency_ex(frames: u32, source: i8) {
    let code = u8::try_from(source).unwrap_or(b'E');
    Engine::set_latency(frames, LatencySource::from_code(code));
}

/// Frames captured but not yet consumed by playback.
#[must_use]
pub extern "C" fn looper_input_backlog() -> u32 {
    Engine::input_backlog()
}

/// Show a short status line.
///
/// # Safety
///
/// `text` must point at a NUL-terminated string, or be null.
pub unsafe extern "C" fn looper_notice(text: *const core::ffi::c_char) {
    if text.is_null() {
        return;
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let bytes = unsafe { std::ffi::CStr::from_ptr(text) }.to_bytes();
    Engine::notice(&String::from_utf8_lossy(bytes));
}

/// Feed one planar S32 capture channel.
///
/// # Safety
///
/// `channels` must point at at least `count` non-null channel pointers and
/// `channels[0]` must be readable for `frames` samples.
pub unsafe extern "C" fn looper_input_s32(channels: *const *const i32, count: i32, frames: u32) {
    if channels.is_null() || count < 1 || frames == 0 {
        return;
    }
    // SAFETY: the caller promises `channels[0]` is readable for `frames`.
    let first = unsafe { ptr::read(channels) };
    if first.is_null() {
        return;
    }
    let len = usize::try_from(frames).unwrap_or(0);
    // SAFETY: see the function contract.
    let slice = unsafe { core::slice::from_raw_parts(first, len) };
    Engine::input_s32(slice);
}

/// Feed interleaved S32 capture frames.
///
/// # Safety
///
/// `buffer` must be readable for `frames * channels` samples.
pub unsafe extern "C" fn looper_input_s32_interleaved(
    buffer: *const i32,
    channels: u32,
    frames: u32,
) {
    if buffer.is_null() || channels == 0 || frames == 0 {
        return;
    }
    let channels = usize::try_from(channels).unwrap_or(0);
    let frames = usize::try_from(frames).unwrap_or(0);
    let Some(len) = channels.checked_mul(frames) else {
        return;
    };
    // SAFETY: see the function contract.
    let slice = unsafe { core::slice::from_raw_parts(buffer, len) };
    Engine::input_s32_interleaved(slice, channels);
}

/// Mix the loops into the interleaved S32 block that is about to be played.
///
/// # Safety
///
/// `buffer` must be writable for `frames * channels` samples.
pub unsafe extern "C" fn looper_render_s32(buffer: *mut i32, channels: u32, frames: u32) {
    if buffer.is_null() || channels == 0 || frames == 0 {
        return;
    }
    let channels = usize::try_from(channels).unwrap_or(0);
    let frames = usize::try_from(frames).unwrap_or(0);
    let Some(len) = channels.checked_mul(frames) else {
        return;
    };
    // SAFETY: see the function contract.
    let slice = unsafe { core::slice::from_raw_parts_mut(buffer, len) };
    Engine::render_s32(slice, channels);
}

/// Snapshot the engine for the paint hook.
///
/// # Safety
///
/// `view` must be writable for one [`View`].
pub unsafe extern "C" fn looper_snapshot(view: *mut View) -> i32 {
    if view.is_null() {
        return 0;
    }
    // SAFETY: see the function contract.
    let view = unsafe { &mut *view };
    i32::from(Engine::snapshot(view))
}
