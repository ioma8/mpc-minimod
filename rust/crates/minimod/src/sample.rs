//! Geometry accessors for the app's sample and instrument objects.
//!
//! Both are pure reads of firmware memory, so they live here as small typed
//! helpers instead of raw pointer arithmetic in the glue.  The offsets are the
//! reverse-engineered ones and change with the firmware.

use crate::firmware;

/// `[sample + 0x130]` is the sample's metadata block.
const SAMPLE_METADATA: usize = 0x130;
/// Shortest and longest sample rate the app accepts.
const RATE_RANGE: core::ops::RangeInclusive<u32> = 8_000..=192_000;

/// The frame count of the sample object, or zero when its metadata is not a
/// usable sample description.
#[must_use]
pub fn frames(sample: usize) -> u32 {
    if sample == 0 {
        return 0;
    }
    let Some(metadata) = sample
        .checked_add(SAMPLE_METADATA)
        .and_then(firmware::read_pointer)
    else {
        return 0;
    };
    let Some(channels) = firmware::read_u32(metadata) else {
        return 0;
    };
    let Ok(channels) = u16::try_from(channels & 0xffff) else {
        return 0;
    };
    if channels != 1 && channels != 2 {
        return 0;
    }
    let (Some(frames), Some(rate)) = (
        firmware::read_u32(metadata + 4),
        firmware::read_u32(metadata + 8),
    ) else {
        return 0;
    };
    if !RATE_RANGE.contains(&rate) {
        return 0;
    }
    frames
}

/// The frame count of a sample object, for the C glue.
///
/// # Safety
///
/// `sample` must be zero or the address of a sample object.
#[must_use]
pub unsafe extern "C" fn frames_of_sample(sample: usize) -> u32 {
    frames(sample)
}

#[cfg(test)]
mod tests {
    use super::RATE_RANGE;

    #[test]
    fn the_rate_range_matches_the_app() {
        assert!(RATE_RANGE.contains(&44_100));
        assert!(RATE_RANGE.contains(&8_000));
        assert!(RATE_RANGE.contains(&192_000));
        assert!(!RATE_RANGE.contains(&7_999));
        assert!(!RATE_RANGE.contains(&192_001));
    }
}
