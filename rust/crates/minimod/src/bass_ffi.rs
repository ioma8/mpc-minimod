//! Entry points for the bass synth, with the C calling convention the hook tables and callbacks use.

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
pub extern "C" fn bass_preview_enable(enabled: i32) {
    bass::preview_enable(enabled != 0);
}

/// Set the three preview knobs, each 0..1.
pub extern "C" fn bass_preview_set_params(brightness: f32, excite: f32, cutoff: f32) {
    bass::preview_set_params(brightness, excite, cutoff);
}

/// Strike one pad for the preview.
pub extern "C" fn bass_preview_note(pad: u32, velocity: f32) {
    if let Ok(pad) = usize::try_from(pad)
        && pad < PADS
    {
        bass::preview_note(pad, velocity);
    }
}

/// Whether the bass preview can run: the note and audio hooks are installed.
pub static mut G_BASS_PREVIEW_AVAILABLE: i32 = 0;

/// How many notes the preview has replayed.
pub static mut G_BASS_PREVIEW_NOTES: u32 = 0;

/// Turn the bass preview on or off, resetting its counters when it starts.
pub extern "C" fn minimod_bass_preview_set_active(active: i32) {
    // SAFETY: only the UI thread writes the flag.
    let available = unsafe { G_BASS_PREVIEW_AVAILABLE } != 0;
    let active = i32::from(active != 0 && available);
    if active != 0 {
        // SAFETY: the counters are this module's.
        unsafe {
            G_BASS_PREVIEW_NOTES = 0;
            crate::audio::G_BASS_PREVIEW_BLOCKS = 0;
            crate::audio::G_BASS_PREVIEW_PEAK_BITS = 0;
        }
    }
    if active != 0 {
        // Enable and clear before the audio thread can see `active`, so the
        // first pad is neither dropped nor mixed into a stale voice.
        bass_preview_enable(1);
        bass::preview_reset();
        // SAFETY: the flag is this module's.
        unsafe { crate::audio::G_BASS_PREVIEW_ACTIVE = 1 };
    } else {
        // SAFETY: the flag is this module's.
        unsafe { crate::audio::G_BASS_PREVIEW_ACTIVE = 0 };
        bass_preview_enable(0);
    }
}
