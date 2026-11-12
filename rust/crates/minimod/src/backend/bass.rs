//! The bass synth and its realtime preview, as safe calls.

use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use minimod_dsp::bass;
pub use minimod_dsp::bass::{Kind, PADS};

/// Whether the preview is audible.
pub static G_BASS_PREVIEW_ACTIVE: AtomicI32 = AtomicI32::new(0);

/// Whether the note hooks the preview needs are installed.
static AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Render the C2 pluck.
///
/// Returns the interleaved stereo audio and its frame count.
#[must_use]
pub fn render(kind: Kind, rate: u32, params: [f32; 3]) -> Option<(Vec<f32>, usize)> {
    bass::render_kind(kind, rate, params)
        .ok()
        .map(|pluck| (pluck.audio, pluck.frames))
}

/// Turn the realtime preview on or off.
pub fn preview_enable(enabled: bool) {
    bass::preview_enable(enabled);
}

/// Set the three preview knobs.
pub fn preview_set_params(harmonics: f32, decay: f32, cutoff: f32) {
    bass::preview_set_params([harmonics, decay, cutoff]);
}

/// The last preview transpose, exported for the device tests.
#[unsafe(no_mangle)]
pub static mut g_bass_transpose: i32 = 0;

/// Shift every preview pad by this many semitones.
pub fn preview_set_transpose(semitones: i32) {
    bass::set_transpose(semitones);
    // SAFETY: the UI thread writes this test-visible mirror.
    unsafe { g_bass_transpose = semitones };
}

/// Select the synth character shown on the control page.
pub fn preview_set_kind(kind: Kind) {
    bass::preview_set_kind(kind);
    let active = preview_active();
    preview_set_active(active);
}

/// Play one preview note.
pub fn preview_note(note: u32, velocity: f32) {
    bass::preview_note(usize::try_from(note).unwrap_or(0), velocity);
}

/// Record whether the note hooks are installed.
pub fn set_available(available: bool) {
    AVAILABLE.store(available, Ordering::Release);
}

/// Whether the preview can play.
#[must_use]
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

/// Whether the preview is audible.
#[must_use]
pub fn preview_active() -> bool {
    G_BASS_PREVIEW_ACTIVE.load(Ordering::Acquire) != 0
}

/// Set whether the preview is audible.
pub fn preview_set_active(active: bool) {
    let on = active && available();
    bass::preview_enable(on);
    G_BASS_PREVIEW_ACTIVE.store(i32::from(on), Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{available, preview_active, preview_set_active, set_available};

    #[test]
    fn the_availability_round_trips() {
        set_available(true);
        assert!(available());
        preview_set_active(true);
        assert!(preview_active());
        preview_set_active(false);
        assert!(!preview_active());
        set_available(false);
        assert!(!available());
        preview_set_active(true);
        assert!(!preview_active(), "unavailable preview stays off");
    }
}
