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
use crate::selection;
use crate::trim;
use crate::wav;

/// The pad the trim falls back to when nothing was pressed.
const FALLBACK_PAD: i32 = 0;

/// Everything the trim reads from the caller.
#[derive(Clone, Copy)]
pub struct Context {
    /// The page's surface.
    pub surface: usize,
    /// The app's control context.
    pub control: usize,
    /// Which press of the cycle this is, 0..=3.
    pub step: i32,
    /// The pad the user pressed last, or -1; only a fallback when the page has
    /// no selected sample.
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

/// Resolve the pad the page currently shows, not the last physical press.
///
/// The page binding holds the selected instrument state at `+0x60`. That is
/// the same object as one entry in the surface's pad table, so matching it
/// returns the selected pad even when the last pad press was another pad or
/// happened before a project/screen change. The selected sample is the fallback
/// when the page state is temporary.
fn selected_pad(surface: usize, last_pad: i32) -> Option<i32> {
    let screen = paint::screen();
    // SAFETY: `screen` is the component the paint hook recorded on this UI
    // thread; `current` validates its Sample-page vtable.
    if let Some(current) = unsafe { selection::current(screen) } {
        if let Some(pad) = pads::pad_for_state(surface, current.state()) {
            return Some(pad);
        }
        if let Some(pad) = pads::pad_for_sample(surface, current.sample()) {
            return Some(pad);
        }
        // A selected sample that is not addressable as a pad has no safe trim
        // target; never fall back to a stale physical press.
        return None;
    }
    (0..128).contains(&last_pad).then_some(last_pad)
}

/// Run one press of the trim cycle.
///
/// The target is the page's selected pad. A physical pad press is only the
/// fallback when the page has no selected sample at all.
pub fn run(context: Context) {
    let Some(pad) = selected_pad(context.surface, context.last_pad) else {
        paint::show_toast("AutoTrim: select a pad");
        return;
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
    // The app's control surface takes f32 frame indices, so a sample longer
    // than 2^24 frames (about 6.3 minutes at 44.1 kHz) rounds the trim to the
    // nearest frame.  That precision is the SDK's control API, not a choice
    // here; the bound is exact for every shorter sample.
    // SAFETY: the control context and surface came from the app's control hook
    // in this UI callback; the pad and values are the trim fields the page owns.
    unsafe {
        crate::control::set_trim(
            context.control,
            context.surface,
            pad,
            first,
            last.saturating_add(1),
        );
    }
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
