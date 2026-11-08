//! C ABI of the firmware effects.

use core::mem::size_of;

use crate::effect::{self, Effect};
use crate::firmware::Firmware;

/// Run one firmware effect over an interleaved f32 buffer.
///
/// On success `*output` is a `malloc`-owned interleaved buffer of
/// `*output_frames * channels` samples that the caller must `free`, and the
/// return value is zero.  Every failure returns -1 and leaves the outputs null.
///
/// # Safety
///
/// `input` must be readable for `frames * channels` floats; `output` and
/// `output_frames` must be writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h3m_effect_render(
    app_base: usize,
    kind: i32,
    input: *const f32,
    frames: usize,
    channels: u32,
    rate: u32,
    output: *mut *mut f32,
    output_frames: *mut usize,
) -> i32 {
    if output.is_null() || output_frames.is_null() {
        return -1;
    }
    // SAFETY: the caller guarantees both pointers are writable.
    unsafe {
        *output = core::ptr::null_mut();
        *output_frames = 0;
    }
    if input.is_null() || frames == 0 {
        return -1;
    }
    let channels = usize::try_from(channels).unwrap_or(0);
    let Some(effect) = Effect::from_code(kind) else {
        return -1;
    };
    // Cross-check the caller's base against the image this process actually has
    // loaded: the loader walk works out the same value the C constructor found,
    // and a disagreement means we would be calling into the wrong image.
    let firmware = match Firmware::find() {
        Some(found) if found.base() != app_base => return -1,
        Some(found) => found,
        None => Firmware::from_base(app_base),
    };
    let Some(len) = channels.checked_mul(frames) else {
        return -1;
    };
    // SAFETY: the caller guarantees `input` is readable for that many floats.
    let input = unsafe { core::slice::from_raw_parts(input, len) };
    let Ok(rendered) = effect::render(firmware, effect, input, channels, rate) else {
        return -1;
    };

    // The caller frees this with libc `free`, so it has to come from libc
    // `malloc` rather than from Rust's allocator.
    let bytes = rendered.audio.len() * size_of::<f32>();
    // SAFETY: a plain allocation, checked for null below.
    let buffer = unsafe { libc::malloc(bytes) }.cast::<f32>();
    if buffer.is_null() {
        return -1;
    }
    // SAFETY: `buffer` holds `bytes` and the source is a live slice of the same
    // length.
    unsafe {
        core::ptr::copy_nonoverlapping(rendered.audio.as_ptr(), buffer, rendered.audio.len());
        *output = buffer;
        *output_frames = rendered.frames;
    }
    0
}
