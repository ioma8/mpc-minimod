//! Geometry accessors for the app's sample and instrument objects.
//!
//! Both are pure reads of firmware memory, so they live here as small typed
//! helpers instead of raw pointer arithmetic in the glue.  The offsets are the
//! reverse-engineered ones and change with the firmware.

use crate::firmware;

/// `[param + 0xb0]` is the context whose `+0x60` is the instrument.
const PARAM_CONTEXT: usize = 0xb0;
/// `[context + 0x60]` is the instrument.
const CONTEXT_INSTRUMENT: usize = 0x60;
/// `[instrument + 0x22e8]` is the layer list.
const INSTRUMENT_LAYERS: usize = 0x22e8;
/// `[sample + 0x130]` is the sample's metadata block.
const SAMPLE_METADATA: usize = 0x130;
/// Shortest and longest sample rate the app accepts.
const RATE_RANGE: core::ops::RangeInclusive<u32> = 8_000..=192_000;

/// The instrument and layer list behind a parameter object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layers {
    /// The layer list.
    pub layers: usize,
    /// The instrument that owns it.
    pub instrument: usize,
}

/// The layers of the instrument that owns `parameter`.
#[must_use]
pub fn layers_of(parameter: usize) -> Option<Layers> {
    let context = firmware::read_pointer(parameter.checked_add(PARAM_CONTEXT)?)?;
    let instrument = firmware::read_pointer(context.checked_add(CONTEXT_INSTRUMENT)?)?;
    let layers = firmware::read_pointer(instrument.checked_add(INSTRUMENT_LAYERS)?)?;
    Some(Layers { layers, instrument })
}

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn frames_of_sample(sample: usize) -> u32 {
    frames(sample)
}

/// The layers behind a parameter object.
///
/// # Safety
///
/// `layers` and `instr` must be writable, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_layers(
    parameter: usize,
    layers: *mut usize,
    instrument: *mut usize,
) -> i32 {
    let Some(found) = layers_of(parameter) else {
        return 0;
    };
    // SAFETY: the caller promises both pointers are writable or null.
    unsafe {
        if !layers.is_null() {
            *layers = found.layers;
        }
        if !instrument.is_null() {
            *instrument = found.instrument;
        }
    }
    1
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
