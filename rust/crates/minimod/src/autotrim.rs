//! The `AutoTrim` soft key: trim the selected pad's recording to its audible
//! bounds, and clear the trim on the fourth press.
//!
//! The bounds come from [`crate::trim`], the file from the page's own path
//! helpers, and the result is applied through the app's control surface so the
//! page redraws its waveform exactly as if the user had moved the markers.

use crate::pads;
use crate::paint;
use crate::paths;
use crate::sample;
use crate::trim;
use crate::wav;

/// The control op that sets the trim's start.
const SET_START: i32 = 0x462;
/// The control op that sets its end.
const SET_END: i32 = 0x466;
/// The pad the trim falls back to when nothing was pressed.
const FALLBACK_PAD: i32 = 0;

/// Everything the trim reads from the caller.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    /// The page's surface.
    pub surface: usize,
    /// The app's control context.
    pub control: usize,
    /// Which press of the cycle this is, 0..=3.
    pub step: i32,
    /// The pad the user pressed last, or -1.
    pub last_pad: i32,
}

/// The thresholds the three first presses use.
const RATIOS: [f64; 3] = [0.03, 0.06, 0.09];

/// The labels the toast shows.
const LABELS: [&str; 4] = [
    "Trim 3% of peak",
    "Trim 6% of peak",
    "Trim 9% of peak",
    "Trim cleared",
];

/// Run one press of the trim cycle.
pub fn run(context: Context) {
    let pad = if (0..128).contains(&context.last_pad) {
        context.last_pad
    } else {
        FALLBACK_PAD
    };
    let Some(sample) = pads::sample_for_pad(context.surface, pad) else {
        return;
    };
    // the sample is the page's own object.
    let frames = sample::frames(sample);
    if frames == 0 {
        return;
    }
    let Some((_, channels, rate)) = sample::geometry(sample) else {
        return;
    };
    let ratio = RATIOS[usize::try_from(context.step).unwrap_or(0).min(2)];
    let (first, last) = if context.step >= 3 {
        (0, frames.saturating_sub(1))
    } else if let Some(found) = bounds(sample, frames, channels, rate, ratio) {
        found
    } else {
        return;
    };
    // The toast has to be armed first: the trim's own repaint is what paints the
    // page, so a toast set afterwards waits for an unrelated frame.
    paint::show_toast(LABELS[usize::try_from(context.step).unwrap_or(3).min(3)]);
    let _ = crate::control::apply(context.control, context.surface, pad, SET_START, 0.0);
    let _ = crate::control::apply(
        context.control,
        context.surface,
        pad,
        SET_END,
        last.saturating_add(1) as f32,
    );
    let _ = crate::control::apply(
        context.control,
        context.surface,
        pad,
        SET_START,
        first as f32,
    );
}

/// The note the selected pad's sample plays at its original speed, C3 when the
/// surface has not been seen or the chain is unreadable.
#[must_use]
pub fn pad_root(pad: i32) -> i32 {
    let pad = if (0..128).contains(&pad) {
        pad
    } else {
        FALLBACK_PAD
    };
    // only the UI thread writes the surface.
    let surface = crate::control::surface();
    if surface == 0 {
        return sample::DEFAULT_ROOT;
    }
    pads::sample_for_pad(surface, pad).map_or(sample::DEFAULT_ROOT, sample::root_note)
}

/// The first and last audible frames at `ratio` of the peak.
///
/// The loaded audio comes first, so a recording that was never saved still
/// trims; a streamed sample falls back to the WAV, which must match the sample's
/// frames, channels and rate.
fn bounds(sample: usize, frames: u32, channels: u16, rate: u32, ratio: f64) -> Option<(u32, u32)> {
    if let Some(memory) = sample::memory(sample)
        && memory.frames == frames
        && let Some(samples) = memory.read()
    {
        let edges = trim::memory_edges(&samples, usize::from(memory.channels), ratio)?;
        return Some((edges.first, edges.last));
    }
    let path = paths::for_sample(sample, frames, channels, rate)?;
    let info = wav::Info::read(&path).filter(|info| info.matches(frames, channels, rate))?;
    let layout = trim::Layout {
        data_offset: info.data_offset,
        channels: info.channels,
        width: info.width,
        encoding: info.encoding,
    };
    let edges = trim::edges(&path, layout, frames, ratio)?;
    Some((edges.first, edges.last))
}

#[cfg(test)]
mod tests {
    use super::{LABELS, RATIOS};

    #[test]
    fn the_cycle_has_a_label_for_every_press() {
        assert_eq!(LABELS.len(), RATIOS.len() + 1);
        assert!(LABELS.iter().all(|label| label.starts_with("Trim")));
    }
}
