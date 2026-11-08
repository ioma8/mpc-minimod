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
const MAX_CHANNELS: i32 = 8;

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
    if !(1..=MAX_CHANNELS).contains(&count) {
        count = 2;
    }
    usize::try_from(count).unwrap_or(2)
}

/// The channel count of one side, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_audio_channels(self_address: usize, input_side: i32) -> i32 {
    i32::try_from(channels(self_address, input_side != 0)).unwrap_or(2)
}

#[cfg(test)]
mod tests {
    use super::{INPUT_ALT, INPUT_CHANNELS, MAX_CHANNELS, OUTPUT_ALT, OUTPUT_CHANNELS, channels};

    #[test]
    fn the_offsets_are_the_documented_ones() {
        assert_eq!(OUTPUT_CHANNELS, 0xd8);
        assert_eq!(OUTPUT_ALT, 0xe4);
        assert_eq!(INPUT_CHANNELS, 0xd4);
        assert_eq!(INPUT_ALT, 0xe0);
        assert_eq!(MAX_CHANNELS, 8);
    }

    #[test]
    fn an_unreadable_object_falls_back_to_stereo() {
        // Address zero is never readable in the target process.
        assert_eq!(channels(0, false), 2);
        assert_eq!(channels(0, true), 2);
    }
}
