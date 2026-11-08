//! C ABI of the WAV helpers.
//!
//! The glue calls these by their pre-port names and signatures, so moving the
//! parsing into Rust did not change a single call site.

use core::mem::size_of;
use core::ptr;
use std::ffi::{CStr, OsStr};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::wav::{self, Encoding};

/// Borrow a NUL-terminated path from C.
///
/// # Safety
///
/// `text` must be a valid NUL-terminated string that outlives the borrow.
unsafe fn c_path<'a>(text: *const c_char) -> Option<&'a Path> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a NUL-terminated string.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    if bytes.is_empty() {
        return None;
    }
    Some(Path::new(OsStr::from_bytes(bytes)))
}

/// Copy a slice into a `malloc` block the caller frees, or null on failure.
fn to_malloc<T: Copy>(values: &[T]) -> *mut T {
    let bytes = values.len().saturating_mul(size_of::<T>());
    // SAFETY: a plain allocation, checked for null.
    let buffer = unsafe { libc::malloc(bytes.max(1)) }.cast::<T>();
    if buffer.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `buffer` holds `bytes` and the source is a live slice.
    unsafe { ptr::copy_nonoverlapping(values.as_ptr(), buffer, values.len()) };
    buffer
}

/// One sample, scaled to the 16-bit range the trim logic compares against.
///
/// # Safety
///
/// `bytes` must be readable for `width` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pcm_at(bytes: *const u8, width: u32, format: u32) -> i32 {
    if bytes.is_null() {
        return 0;
    }
    let Ok(width) = u16::try_from(width) else {
        return 0;
    };
    if width == 0 || width > 8 {
        return 0;
    }
    let encoding = if format == 3 {
        Encoding::Float
    } else {
        Encoding::Pcm
    };
    // SAFETY: the caller promises `bytes` is readable for `width` bytes.
    let slice = unsafe { core::slice::from_raw_parts(bytes, usize::from(width)) };
    wav::scaled_sample(slice, width, encoding)
}

/// Read a WAV header.
///
/// # Safety
///
/// `path` must be a NUL-terminated string; every out-pointer may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wav_info(
    path: *const c_char,
    frames: *mut u32,
    rate: *mut u32,
    channels: *mut u32,
    data_offset: *mut u32,
    bytes_per_sample: *mut u32,
    format: *mut u32,
) -> i32 {
    // SAFETY: see the function contract.
    let Some(path) = (unsafe { c_path(path) }) else {
        return 0;
    };
    let Some(info) = wav::Info::read(path) else {
        return 0;
    };
    // SAFETY: the caller promises each pointer is writable or null.
    unsafe {
        if !frames.is_null() {
            *frames = info.frames;
        }
        if !rate.is_null() {
            *rate = info.rate;
        }
        if !channels.is_null() {
            *channels = u32::from(info.channels);
        }
        if !data_offset.is_null() {
            *data_offset = u32::try_from(info.data_offset).unwrap_or(u32::MAX);
        }
        if !bytes_per_sample.is_null() {
            *bytes_per_sample = u32::from(info.width);
        }
        if !format.is_null() {
            *format = match info.encoding {
                Encoding::Pcm => 1,
                Encoding::Float => 3,
            };
        }
    }
    1
}

/// Decode a WAV file into an interleaved f32 buffer the caller `free`s.
///
/// # Safety
///
/// `path` must be a NUL-terminated string; `out` and the optional outputs must
/// be writable or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wav_read_f32(
    path: *const c_char,
    want_frames: u32,
    want_channel_count: u32,
    want_rate: u32,
    out: *mut *mut f32,
    got_channels: *mut u32,
    got_rate: *mut u32,
) -> i32 {
    if out.is_null() {
        return 0;
    }
    // SAFETY: the caller promised `out` is writable.
    unsafe { *out = ptr::null_mut() };
    // SAFETY: see the function contract.
    let Some(path) = (unsafe { c_path(path) }) else {
        return 0;
    };
    let Ok(channels) = u16::try_from(want_channel_count) else {
        return 0;
    };
    let Some(samples) = wav::read_f32(path, want_frames, channels, want_rate) else {
        return 0;
    };
    let buffer = to_malloc(&samples);
    if buffer.is_null() {
        return 0;
    }
    // SAFETY: the caller promises the optional outputs are writable.
    unsafe {
        *out = buffer;
        if !got_channels.is_null() {
            *got_channels = u32::from(channels);
        }
        if !got_rate.is_null() {
            *got_rate = want_rate;
        }
    }
    1
}

/// Copy the `data` chunk of a WAV so it can be put back after the firmware
/// rewrote it.
///
/// # Safety
///
/// `path` must be a NUL-terminated string; the outputs must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bass_source_backup(
    path: *const c_char,
    data_offset: *mut i64,
    data: *mut *mut u8,
    data_size: *mut usize,
) -> i32 {
    if data_offset.is_null() || data.is_null() || data_size.is_null() {
        return 0;
    }
    // SAFETY: the caller promises the outputs are writable.
    unsafe {
        *data = ptr::null_mut();
        *data_size = 0;
    }
    // SAFETY: see the function contract.
    let Some(path) = (unsafe { c_path(path) }) else {
        return 0;
    };
    let Some((offset, chunk)) = wav::data_chunk(path) else {
        return 0;
    };
    let buffer = to_malloc(&chunk);
    if buffer.is_null() {
        return 0;
    }
    // SAFETY: the caller promises the outputs are writable.
    unsafe {
        *data_offset = i64::try_from(offset).unwrap_or(0);
        *data = buffer;
        *data_size = chunk.len();
    }
    1
}

/// Write a previously backed-up `data` chunk back into a WAV.
///
/// # Safety
///
/// `path` must be a NUL-terminated string and `data` readable for `data_size`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bass_source_restore(
    path: *const c_char,
    data_offset: i64,
    data: *const u8,
    data_size: usize,
) -> i32 {
    if data.is_null() || data_size == 0 {
        return 0;
    }
    // SAFETY: see the function contract.
    let Some(path) = (unsafe { c_path(path) }) else {
        return 0;
    };
    let Ok(offset) = u64::try_from(data_offset) else {
        return 0;
    };
    // SAFETY: the caller promises `data` is readable for `data_size` bytes.
    let chunk = unsafe { core::slice::from_raw_parts(data, data_size) };
    i32::from(wav::restore_data(path, offset, chunk).is_ok())
}
