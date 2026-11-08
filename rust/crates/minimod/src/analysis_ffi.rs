//! C ABI of the onset detector.
//!
//! The detector takes interleaved f32 and fills a caller-owned position array,
//! exactly as the glue called it before the port.

use minimod_dsp::spectral;

/// Detect the onsets in interleaved audio.
///
/// # Safety
///
/// `input` must be readable for `frames * channels` samples; `positions` must be
/// writable for `max_positions` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn spectral_onsets(
    input: *const f32,
    frames: usize,
    channels: u32,
    rate: u32,
    positions: *mut u32,
    max_positions: u32,
    detected: *mut u32,
) -> u32 {
    if input.is_null() || positions.is_null() {
        return 0;
    }
    let Ok(channels) = usize::try_from(channels) else {
        return 0;
    };
    let Ok(max_positions) = usize::try_from(max_positions) else {
        return 0;
    };
    let Some(samples) = frames.checked_mul(channels) else {
        return 0;
    };
    // SAFETY: the caller promises `input` is readable for that many samples.
    let input = unsafe { core::slice::from_raw_parts(input, samples) };
    let Ok(onsets) = spectral::detect(input, channels, rate, max_positions) else {
        return 0;
    };
    let count = onsets.positions.len().min(max_positions);
    // SAFETY: the caller promises `positions` is writable for `max_positions`.
    unsafe {
        core::ptr::copy_nonoverlapping(onsets.positions.as_ptr(), positions, count);
        if !detected.is_null() {
            *detected = onsets.detected;
        }
    }
    u32::try_from(count).unwrap_or(0)
}
