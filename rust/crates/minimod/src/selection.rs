//! Which sample the Sample page has selected.
//!
//! Everything the sample actions do starts here: find the screen's binding, walk
//! to the instrument, its layer list and the selected layer's sample, then read
//! the sample's own geometry.  A stale pointer anywhere in that chain means "no
//! sample", never a guess.

use crate::firmware;

/// The Sample page's vtable, as an offset from the image base.
pub const SCREEN_VTABLE: usize = 0x6e2_12a0;
/// `[screen + 0x178]` is the page's binding.
const SCREEN_BINDING: usize = 0x178;
/// `[binding + 0x60]` is the state the instrument lives in.
const BINDING_STATE: usize = 0x60;
/// `[state + 0x22e8]` is the layer list.
const STATE_LAYERS: usize = 0x22e8;
/// `[layers + 0x4a8]` is the selected sample.
const LAYERS_SAMPLE: usize = 0x4a8;
/// `[sample + 0x130]` is its metadata block.
const SAMPLE_METADATA: usize = 0x130;
/// The app's "is this page showing a sample" predicate.
const PAGE_HAS_SAMPLE: usize = 0x112_fd50;
/// Shortest and longest sample rate the app accepts.
const RATE_RANGE: core::ops::RangeInclusive<u32> = 8_000..=192_000;

/// The selected sample and where it was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The screen the walk started from.
    pub screen: usize,
    /// The page state the instrument lives in.
    pub state: usize,
    /// The selected sample object.
    pub sample: usize,
    /// Its length in frames.
    pub frames: u32,
    /// Its channel count.
    pub channels: u16,
    /// Its sample rate.
    pub rate: u32,
}

/// The sample the Sample page has selected, or `None`.
#[must_use]
pub fn current(base: usize, screen: usize) -> Option<Selection> {
    if screen == 0 {
        return None;
    }
    if firmware::read_pointer(screen)? != base + SCREEN_VTABLE {
        return None;
    }
    // SAFETY: `base + PAGE_HAS_SAMPLE` is the page's own predicate and `screen`
    // is a Sample page.
    if unsafe { firmware::int_of_pointer(base + PAGE_HAS_SAMPLE, screen) } == 0 {
        return None;
    }
    let binding = firmware::read_pointer(screen.checked_add(SCREEN_BINDING)?)?;
    let state = firmware::read_pointer(binding.checked_add(BINDING_STATE)?)?;
    let layers = firmware::read_pointer(state.checked_add(STATE_LAYERS)?)?;
    let sample = firmware::read_pointer(layers.checked_add(LAYERS_SAMPLE)?)?;
    let metadata = firmware::read_pointer(sample.checked_add(SAMPLE_METADATA)?)?;
    let channels = u16::try_from(firmware::read_u32(metadata)? & 0xffff).ok()?;
    let frames = firmware::read_u32(metadata.checked_add(4)?)?;
    let rate = firmware::read_u32(metadata.checked_add(8)?)?;
    if !matches!(channels, 1 | 2) || frames == 0 || !RATE_RANGE.contains(&rate) {
        return None;
    }
    Some(Selection {
        screen,
        state,
        sample,
        frames,
        channels,
        rate,
    })
}

/// The selected sample, for the C glue.
///
/// Writes the found pointers and returns 1; leaves the outputs untouched and
/// returns 0 when no sample is selected.
///
/// # Safety
///
/// Every out-pointer must be writable, or null.
pub unsafe extern "C" fn minimod_sample_now(
    base: usize,
    screen: usize,
    screen_out: *mut usize,
    state: *mut usize,
    sample: *mut usize,
    frames: *mut u32,
    channels: *mut u32,
    rate: *mut u32,
) -> i32 {
    let Some(found) = current(base, screen) else {
        return 0;
    };
    // SAFETY: the caller promises every out-pointer is writable or null.
    unsafe {
        if !screen_out.is_null() {
            *screen_out = found.screen;
        }
        if !state.is_null() {
            *state = found.state;
        }
        if !sample.is_null() {
            *sample = found.sample;
        }
        if !frames.is_null() {
            *frames = found.frames;
        }
        if !channels.is_null() {
            *channels = u32::from(found.channels);
        }
        if !rate.is_null() {
            *rate = found.rate;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::{RATE_RANGE, SCREEN_VTABLE};

    #[test]
    fn the_screen_vtable_is_the_documented_one() {
        assert_eq!(SCREEN_VTABLE, 0x6e2_12a0);
        assert!(RATE_RANGE.contains(&44_100));
        assert!(!RATE_RANGE.contains(&4_000));
        // A zero screen is never a selection.
        assert!(super::current(0x1000_0000, 0).is_none());
    }
}
