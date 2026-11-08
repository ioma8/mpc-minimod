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

/// The app's slice Reset.
const SLICE_RESET: usize = 0x295_0aa0;
/// The app's slice Insert.
const SLICE_INSERT: usize = 0x295_0d20;
/// The app's "reconcile the last slice's end" setter.
const SLICE_SET_END: usize = 0x295_0bb0;
/// The app's selection setter.
const SELECT_SLICE: usize = 0x166_80a0;
/// `[screen + 0x180]` is the view's selection object.
const SCREEN_SELECTION: usize = 0x180;
/// `[selection + 0x618]` is where the selection index goes.
const SELECTION_INDEX: usize = 0x618;

/// Write the requested boundaries through the app's own Reset/Insert calls and
/// check the result back.
///
/// The app owns the slice allocation, its listeners and the broadcasts, so the
/// only safe way to chop is to drive its calls in order and then read the
/// result: a short write shows up as a mismatch in [`verify`], never as a
/// quietly wrong pad.
#[must_use]
pub fn write_markers(
    base: usize,
    sample: usize,
    positions: &[u32],
    frames: u32,
    screen: usize,
) -> bool {
    let Some(slices) = slice_count(sample) else {
        return false;
    };
    let Some(selection) = firmware::read_pointer(screen.checked_add(SCREEN_SELECTION).unwrap_or(0))
    else {
        return false;
    };
    if selection == 0 {
        return false;
    }
    // SAFETY: the offsets are the app's own entries, called the way the stock
    // Chop UI calls them.
    unsafe {
        firmware::void_of_pointer(base + SLICE_RESET, slices.continuation);
        for (index, position) in positions.iter().enumerate() {
            let assigned = firmware::insert_slice(
                base + SLICE_INSERT,
                slices.continuation,
                *position,
                1,
                frames,
            );
            if usize::try_from(assigned).unwrap_or(usize::MAX) != index {
                return false;
            }
        }
        firmware::void_of_pointer_value(base + SLICE_SET_END, slices.continuation, frames);
        let mut first: i32 = 0;
        firmware::void_of_two_pointers(
            base + SELECT_SLICE,
            selection + SELECTION_INDEX,
            core::ptr::addr_of_mut!(first) as usize,
        );
    }
    verify(sample, positions, frames)
}

/// Write the boundaries through the app and verify them, for the C glue.
///
/// # Safety
///
/// `positions` must be readable for `count` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_spectral_write_markers(
    base: usize,
    sample: usize,
    positions: *const u32,
    count: u32,
    frames: u32,
    screen: usize,
) -> i32 {
    if positions.is_null() {
        return 0;
    }
    let Ok(count) = usize::try_from(count) else {
        return 0;
    };
    // SAFETY: the caller promises `positions` is readable for `count` entries.
    let positions = unsafe { core::slice::from_raw_parts(positions, count) };
    i32::from(write_markers(base, sample, positions, frames, screen))
}

/// The view's cached chop mode callback, used when the binding is already torn
/// down.
const SET_CHOP_MODE: usize = 0x15d_2840;
/// `[screen + 0x168]` is the chop binding.
const SCREEN_BINDING: usize = 0x168;
/// The binding's flags byte, owner, manager and invoke slot.
const BINDING_FLAGS: usize = 0xe8;
const BINDING_OWNER: usize = 0xe0;
const BINDING_MANAGER: usize = 0xd0;
const BINDING_INVOKE: usize = 0xd8;
/// The model callback the binding is driven with, and its argument object.
const BINDING_CALL: usize = 0xc0;
const BINDING_FALLBACK: usize = 0x80;

/// Put the page's chop mode back to Manual.
///
/// The stock Split handler drives the binding's model callback; assigning the
/// view's cached enum alone does not take, which is why this goes through the
/// callback whenever the binding is still alive.
#[must_use]
pub fn set_manual(base: usize, screen: usize) -> bool {
    let Some(binding) = screen
        .checked_add(SCREEN_BINDING)
        .and_then(firmware::read_pointer)
    else {
        return false;
    };
    let Some(owner) = binding
        .checked_add(BINDING_OWNER)
        .and_then(firmware::read_pointer)
    else {
        return false;
    };
    let Some(flags) = firmware::read_u32(binding.saturating_add(BINDING_FLAGS)) else {
        return false;
    };
    if flags & 1 == 0 && owner != 0 {
        let Some(manager) = binding
            .checked_add(BINDING_MANAGER)
            .and_then(firmware::read_pointer)
        else {
            return false;
        };
        let Some(invoke) = binding
            .checked_add(BINDING_INVOKE)
            .and_then(firmware::read_pointer)
        else {
            return false;
        };
        if manager == 0 || invoke == 0 {
            return false;
        }
        let mut manual: i32 = 0;
        // SAFETY: the binding's invoke slot is the app's model callback.
        unsafe {
            firmware::void_of_three_pointers(
                invoke,
                binding + BINDING_CALL,
                owner,
                core::ptr::addr_of_mut!(manual) as usize,
            );
        }
    } else {
        // SAFETY: the fallback is the app's own cached-mode setter.
        unsafe {
            firmware::void_of_pointer_value(base + SET_CHOP_MODE, binding + BINDING_FALLBACK, 0);
        }
    }
    true
}

/// Put the page's chop mode back to Manual, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_spectral_manual(base: usize, screen: usize) -> i32 {
    i32::from(set_manual(base, screen))
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
