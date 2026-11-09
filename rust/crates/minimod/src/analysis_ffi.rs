//! Entry points for the two analysis algorithms, with the C calling convention the hook tables and callbacks use.
//!
//! The split takes interleaved f32 and hands back two `malloc`-owned buffers, and
//! the onset detector fills a caller-owned position array; both are what the glue
//! called before the port.

use core::mem::size_of;
use core::ptr;

use minimod_dsp::hpss;

/// Copy a slice into a `malloc` block the caller frees, or null on failure.
fn to_malloc(values: &[f32]) -> *mut f32 {
    let bytes = values.len().saturating_mul(size_of::<f32>());
    // SAFETY: a plain allocation, checked for null.
    let buffer = unsafe { libc::malloc(bytes.max(1)) }.cast::<f32>();
    if buffer.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `buffer` holds `bytes` and the source is a live slice.
    unsafe { ptr::copy_nonoverlapping(values.as_ptr(), buffer, values.len()) };
    buffer
}

/// Split interleaved audio into harmonic and percussive parts.
///
/// Returns 0 on success and -1 on failure, matching the reference's convention
/// (the caller treats a non-zero result as a failed job).
///
/// # Safety
///
/// `input` must be readable for `frames * channels` samples, and each output
/// pointer must be writable.
pub unsafe extern "C" fn hpss_split(
    input: *const f32,
    frames: usize,
    channels: u32,
    rate: u32,
    harmonic: *mut *mut f32,
    percussive: *mut *mut f32,
) -> i32 {
    if input.is_null() || harmonic.is_null() || percussive.is_null() {
        return -1;
    }
    // SAFETY: the caller promises the outputs are writable.
    unsafe {
        *harmonic = ptr::null_mut();
        *percussive = ptr::null_mut();
    }
    let Ok(channels) = usize::try_from(channels) else {
        return -1;
    };
    let Some(samples) = frames.checked_mul(channels) else {
        return -1;
    };
    // SAFETY: the caller promises `input` is readable for that many samples.
    let input = unsafe { core::slice::from_raw_parts(input, samples) };
    let Ok((harmonic_part, percussive_part)) = hpss::split(input, channels, rate) else {
        return -1;
    };
    let first = to_malloc(&harmonic_part);
    let second = to_malloc(&percussive_part);
    if first.is_null() || second.is_null() {
        // SAFETY: both came from `malloc` and nothing else owns them.
        unsafe {
            libc::free(first.cast());
            libc::free(second.cast());
        }
        return -1;
    }
    // SAFETY: the caller promises the outputs are writable.
    unsafe {
        *harmonic = first;
        *percussive = second;
    }
    0
}
