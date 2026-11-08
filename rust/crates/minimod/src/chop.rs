//! The slice list the app keeps on a chopped sample.
//!
//! Spectral Chop writes its boundaries through the app's own Reset/Insert calls,
//! which own the slice allocation.  Reading the result back is a pure object
//! walk: the sample's metadata block holds a vector of slice pointers, and each
//! slice carries its start and end in frames.

use crate::firmware;

/// `[sample + 0x130]` is the sample's metadata block.
const SAMPLE_METADATA: usize = 0x130;
/// `[metadata + 0x70..0x80]` is the slice vector: begin, end, capacity.
const SLICES_BEGIN: usize = 0x70;
const SLICES_END: usize = 0x78;
const SLICES_CAPACITY: usize = 0x80;
/// `[metadata + 0x10]` is what the app's Insert/Reset calls take.
const SLICES_CONTINUATION: usize = 0x10;
/// A slice pointer is 8 bytes and the app never keeps more than this many.
const SLICE_BYTES: u64 = 8;
const MAX_SLICES: u64 = 128;
/// `[slice + 0x78]` is its first frame, `+0xd0` its end.
const SLICE_START: usize = 0x78;
const SLICE_END: usize = 0xd0;

/// The slice vector of a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slices {
    /// How many slices the app has.
    pub count: u32,
    /// The first slice pointer.
    pub begin: usize,
    /// The object to hand to the app's Insert/Reset calls.
    pub continuation: usize,
}

/// Read the slice vector, or `None` when the sample is not chopped.
#[must_use]
pub fn slice_count(sample: usize) -> Option<Slices> {
    if sample == 0 {
        return None;
    }
    let metadata = firmware::read_pointer(sample.checked_add(SAMPLE_METADATA)?)?;
    let begin = firmware::read_pointer(metadata.checked_add(SLICES_BEGIN)?)?;
    let end = firmware::read_pointer(metadata.checked_add(SLICES_END)?)?;
    let capacity = firmware::read_pointer(metadata.checked_add(SLICES_CAPACITY)?)?;
    if begin == 0 || end < begin || capacity < end {
        return None;
    }
    let span = u64::try_from(end - begin).ok()?;
    let count = span / SLICE_BYTES;
    if span % SLICE_BYTES != 0 || count > MAX_SLICES {
        return None;
    }
    Some(Slices {
        count: u32::try_from(count).ok()?,
        begin,
        continuation: metadata.checked_add(SLICES_CONTINUATION)?,
    })
}

/// Whether the app's slices start and end exactly where the requested
/// boundaries say, with the last one running to the end of the sample.
#[must_use]
pub fn verify(sample: usize, positions: &[u32], frames: u32) -> bool {
    let Some(slices) = slice_count(sample) else {
        return false;
    };
    if usize::try_from(slices.count).unwrap_or(usize::MAX) != positions.len() {
        return false;
    }
    for (index, position) in positions.iter().enumerate() {
        let Some(slice) = firmware::read_pointer(slices.begin + index * 8) else {
            return false;
        };
        let (Some(start), Some(end)) = (
            firmware::read_u32(slice + SLICE_START),
            firmware::read_u32(slice + SLICE_END),
        ) else {
            return false;
        };
        let wanted_end = positions.get(index + 1).copied().unwrap_or(frames);
        if start != *position || end != wanted_end {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{MAX_SLICES, SAMPLE_METADATA, SLICE_BYTES, SLICE_END, SLICE_START};

    #[test]
    fn the_object_layout_matches_the_reverse_engineering() {
        assert_eq!(SAMPLE_METADATA, 0x130);
        assert_eq!(SLICE_BYTES, 8);
        assert_eq!(MAX_SLICES, 128);
        assert_eq!(SLICE_START, 0x78);
        assert_eq!(SLICE_END, 0xd0);
    }

    #[test]
    fn a_zero_sample_has_no_slices() {
        assert!(super::slice_count(0).is_none());
        assert!(!super::verify(0, &[0], 100));
    }
}

/// The slice count of a sample, with the two pointers the app's Insert/Reset
/// calls need.  Returns 0 when the sample is not chopped.
///
/// # Safety
///
/// `begin` and `continuation` must be writable or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_slice_count(
    sample: usize,
    begin: *mut usize,
    continuation: *mut usize,
) -> u32 {
    let Some(slices) = slice_count(sample) else {
        return 0;
    };
    // SAFETY: the caller promises both pointers are writable or null.
    unsafe {
        if !begin.is_null() {
            *begin = slices.begin;
        }
        if !continuation.is_null() {
            *continuation = slices.continuation;
        }
    }
    slices.count
}

/// Whether the app's slices match `positions`, with the last ending at `frames`.
///
/// # Safety
///
/// `positions` must be readable for `count` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_spectral_verify(
    sample: usize,
    positions: *const u32,
    count: u32,
    frames: u32,
) -> i32 {
    if positions.is_null() {
        return 0;
    }
    let Ok(count) = usize::try_from(count) else {
        return 0;
    };
    // SAFETY: the caller promises `positions` is readable for `count` entries.
    let positions = unsafe { core::slice::from_raw_parts(positions, count) };
    i32::from(verify(sample, positions, frames))
}
