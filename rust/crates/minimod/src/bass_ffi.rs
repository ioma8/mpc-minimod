//! C ABI of the bass synth.

use core::mem::size_of;

use minimod_dsp::bass::{self, PADS};

/// Render the C2 pluck.
///
/// On success `*output` is a `malloc`-owned interleaved stereo buffer of
/// `*output_frames * 2` samples that the caller must `free`, and the return
/// value is zero.  Every failure returns -1 and leaves the outputs null.
///
/// # Safety
///
/// `output` and `output_frames` must be writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bass_sample_render(
    rate: u32,
    harmonics: f32,
    drive: f32,
    cutoff: f32,
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
    let Ok(pluck) = bass::render(rate, harmonics, drive, cutoff) else {
        return -1;
    };
    // The caller frees this with libc `free`, so it has to come from libc
    // `malloc` rather than from Rust's allocator.
    let bytes = pluck.audio.len() * size_of::<f32>();
    // SAFETY: a plain allocation, checked for null below.
    let buffer = unsafe { libc::malloc(bytes) }.cast::<f32>();
    if buffer.is_null() {
        return -1;
    }
    // SAFETY: `buffer` holds `bytes` and the source is a live slice of the same
    // length.
    unsafe {
        core::ptr::copy_nonoverlapping(pluck.audio.as_ptr(), buffer, pluck.audio.len());
        *output = buffer;
        *output_frames = pluck.frames;
    }
    0
}

/// Turn the realtime preview on or off.
#[unsafe(no_mangle)]
pub extern "C" fn bass_preview_enable(enabled: i32) {
    bass::preview_enable(enabled != 0);
}

/// Set the three preview knobs, each 0..1.
#[unsafe(no_mangle)]
pub extern "C" fn bass_preview_set_params(brightness: f32, excite: f32, cutoff: f32) {
    bass::preview_set_params(brightness, excite, cutoff);
}

/// Strike one pad for the preview.
#[unsafe(no_mangle)]
pub extern "C" fn bass_preview_note(pad: u32, velocity: f32) {
    if let Ok(pad) = usize::try_from(pad)
        && pad < PADS
    {
        bass::preview_note(pad, velocity);
    }
}

/// Mix the preview into the device's output channels and return its peak.
///
/// # Safety
///
/// Every non-null entry of `channels` must be writable for `frames` samples.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bass_preview_mix(
    channels: *mut *mut f32,
    frames: usize,
    channel_count: u32,
    rate: u32,
) -> f32 {
    if channels.is_null() || frames == 0 || channel_count == 0 {
        return 0.0;
    }
    let count = usize::try_from(channel_count).unwrap_or(0);
    // SAFETY: the caller guarantees each channel pointer is writable.
    unsafe {
        let first = *channels;
        if first.is_null() {
            return 0.0;
        }
        if count == 1 {
            return bass::preview_mix(&mut [first], frames, rate);
        }
        let second = *channels.add(1);
        if second.is_null() {
            return bass::preview_mix(&mut [first], frames, rate);
        }
        bass::preview_mix(&mut [first, second], frames, rate)
    }
}

/// Whether the bass preview can run: the note and audio hooks are installed.
#[unsafe(no_mangle)]
pub static mut g_bass_preview_available: i32 = 0;

/// How many notes the preview has replayed.
#[unsafe(no_mangle)]
pub static mut g_bass_preview_notes: u32 = 0;

/// Turn the bass preview on or off, resetting its counters when it starts.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_bass_preview_set_active(active: i32) {
    // SAFETY: only the UI thread writes the flag.
    let available = unsafe { g_bass_preview_available } != 0;
    let active = i32::from(active != 0 && available);
    if active != 0 {
        // SAFETY: the counters are this module's.
        unsafe {
            g_bass_preview_notes = 0;
            crate::audio::g_bass_preview_blocks = 0;
            crate::audio::g_bass_preview_peak_bits = 0;
        }
    }
    // SAFETY: the flag is this module's.
    unsafe { crate::audio::g_bass_preview_active = active };
    bass_preview_enable(active);
}
