//! Reading the device's stream geometry off the app's audio object.
//!
//! The app keeps the channel count of each stream in its own object, sometimes
//! in a second field when the first is negative.  Getting this wrong would mix
//! the wrong number of channels, so the fallback to stereo is explicit.

use crate::firmware;

/// `[self + 0xd8]` is the output channel count, `+0xe4` its alternative.
const OUTPUT_CHANNELS: usize = 0xd8;
const OUTPUT_ALT: usize = 0xe4;
/// `[self + 0xd4]` is the input channel count, `+0xe0` its alternative.
const INPUT_CHANNELS: usize = 0xd4;
const INPUT_ALT: usize = 0xe0;
/// The device never has more channels than this.
const CHANNEL_LIMIT: i32 = 8;

/// The channel count of one side of the app's audio object.
#[must_use]
pub fn channels(self_address: usize, input_side: bool) -> usize {
    let read = |offset: usize| -> i32 {
        firmware::read_u32(self_address.wrapping_add(offset)).map_or(-1, u32::cast_signed)
    };
    let mut count = if input_side {
        let primary = read(INPUT_CHANNELS);
        if primary < 0 {
            read(INPUT_ALT)
        } else {
            primary
        }
    } else {
        let primary = read(OUTPUT_CHANNELS);
        if primary < 0 {
            read(OUTPUT_ALT)
        } else {
            primary
        }
    };
    if !(1..=CHANNEL_LIMIT).contains(&count) {
        count = 2;
    }
    usize::try_from(count).unwrap_or(2)
}

/// The channel count of one side, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_audio_channels(self_address: usize, input_side: i32) -> i32 {
    i32::try_from(channels(self_address, input_side != 0)).unwrap_or(2)
}

/// Frames the writer will ever see in one block.
pub const MAX_FRAMES: usize = 4096;
/// Channels the writer will ever see.
pub const MAX_CHANNELS: usize = 8;

/// The app's writer: closure, channel array, frames, offset.
pub type Writer = unsafe extern "C" fn(usize, usize, usize, i32);

/// The copy the mod hands to the app when it mixed something in.
///
/// Flat and heap-allocated on first use rather than a thread-local: a 128 KiB
/// thread-local lands in `.tbss` and pushed `.bss` outside the loader's last
/// segment, so the mod's own globals were addressed past the end of its image.
static SCRATCH: std::sync::Mutex<Option<Box<[f32]>>> = std::sync::Mutex::new(None);

/// Run `body` with the scratch buffer, allocating it on first use.
fn with_scratch<T>(body: impl FnOnce(&mut [f32]) -> T) -> Option<T> {
    let mut guard = SCRATCH.lock().ok()?;
    let buffer = guard.get_or_insert_with(|| vec![0.0_f32; MAX_FRAMES * MAX_CHANNELS].into());
    Some(body(buffer))
}

/// Mix the preview into the app's block and forward it.
///
/// Returns the preview's peak when it produced audio, so the caller can keep its
/// statistics without the writer carrying them.
///
/// # Safety
///
/// `input` must be an array of `channels(self_address, false)` writable channel
/// pointers, each valid for `frames` samples, and `orig` must be the writer the
/// hook displaced.
pub unsafe fn write(
    self_address: usize,
    input: *mut *mut f32,
    frames: usize,
    offset: i32,
    orig: Option<Writer>,
    preview: bool,
) -> Option<f32> {
    if !preview {
        // Clear ringing voices after the menu closes, then pass the block on.
        // SAFETY: an empty channel list mixes nothing.
        unsafe { minimod_dsp::bass::preview_mix(&mut [], 0, 44_100) };
        forward(orig, self_address, input, frames, offset);
        return None;
    }
    let count = channels(self_address, false);
    if input.is_null() || frames == 0 || frames > MAX_FRAMES || !(1..=MAX_CHANNELS).contains(&count)
    {
        forward(orig, self_address, input, frames, offset);
        return None;
    }
    let peak = with_scratch(|scratch| {
        let mut pointers: [*mut f32; MAX_CHANNELS] = [core::ptr::null_mut(); MAX_CHANNELS];
        for (index, slot) in pointers.iter_mut().enumerate().take(count) {
            // SAFETY: the caller promises each channel pointer is readable.
            let source = unsafe { *input.add(index) };
            if source.is_null() {
                return None;
            }
            let channel = &mut scratch[index * MAX_FRAMES..][..frames];
            // SAFETY: the source is readable for `frames` samples and the
            // destination is a slice of exactly that length.
            unsafe {
                core::ptr::copy_nonoverlapping(source, channel.as_mut_ptr(), frames);
            }
            *slot = channel.as_mut_ptr();
        }
        // SAFETY: every pointer in the array points at `frames` writable samples.
        let peak =
            unsafe { minimod_dsp::bass::preview_mix(&mut pointers[..count], frames, 44_100) };
        Some((pointers, peak))
    });
    let Some(Some((mut pointers, peak))) = peak else {
        forward(orig, self_address, input, frames, offset);
        return None;
    };
    forward(orig, self_address, pointers.as_mut_ptr(), frames, offset);
    (peak > 0.0).then_some(peak)
}

/// Forward a block to the app's own writer.
fn forward(
    orig: Option<Writer>,
    self_address: usize,
    channels: *mut *mut f32,
    frames: usize,
    offset: i32,
) {
    if let Some(writer) = orig {
        // SAFETY: the caller guarantees this is the displaced writer and the
        // block matches what the app gave it.
        unsafe { writer(self_address, channels as usize, frames, offset) };
    }
}

/// The audio writer, for the C glue.
///
/// # Safety
///
/// See [`write`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_audio_writer(
    self_address: usize,
    input: *mut *mut f32,
    frames: usize,
    offset: i32,
    orig: Option<Writer>,
    preview: i32,
    peak_out: *mut f32,
) -> i32 {
    // SAFETY: see the function contract.
    let peak = unsafe { write(self_address, input, frames, offset, orig, preview != 0) };
    match peak {
        Some(value) => {
            if !peak_out.is_null() {
                // SAFETY: the caller promises `peak_out` is writable.
                unsafe { *peak_out = value };
            }
            1
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{CHANNEL_LIMIT, INPUT_ALT, INPUT_CHANNELS, OUTPUT_ALT, OUTPUT_CHANNELS, channels};

    #[test]
    fn the_offsets_are_the_documented_ones() {
        assert_eq!(OUTPUT_CHANNELS, 0xd8);
        assert_eq!(OUTPUT_ALT, 0xe4);
        assert_eq!(INPUT_CHANNELS, 0xd4);
        assert_eq!(INPUT_ALT, 0xe0);
        assert_eq!(CHANNEL_LIMIT, 8);
    }

    #[test]
    fn an_unreadable_object_falls_back_to_stereo() {
        // Address zero is never readable in the target process.
        assert_eq!(channels(0, false), 2);
        assert_eq!(channels(0, true), 2);
    }
}
