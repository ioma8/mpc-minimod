//! Purple chromatic pad lighting while the synth control page is open.
//!
//! The synth preview plays the selected character chromatically from the pads.
//! Pad 1 is off, pad 16 is full brightness, and the sixteen pads share one
//! purple hue; the app's native lighting resumes when the page closes.

use mpc_sample_sdk::platform::lighting;

/// The shared purple hue.
const PURPLE: u32 = 0x009b_30ff;
/// Number of pads in the chromatic layout.
const PADS: u8 = 16;

/// Light the sixteen pads in the synth gradient.
pub fn show() {
    for pad in 0..PADS {
        let brightness = f32::from(pad) / f32::from(PADS - 1);
        let _ = lighting::send_pad_light(pad, PURPLE, brightness);
    }
}

/// Return every pad to the app's native colour.
pub fn hide() {
    lighting::restore_pad_rgbs();
}
