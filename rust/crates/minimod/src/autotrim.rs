//! The `AutoTrim` soft key: trim the selected pad's recording to its audible
//! bounds, and clear the trim on the fourth press.
//!
//! The bounds come from [`crate::trim`], the file from the page's own path
//! helpers, and the result is applied through the app's control surface so the
//! page redraws its waveform exactly as if the user had moved the markers.

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

/// Where the page keeps its pad array, from the surface.
const SURFACE_PADS: usize = 0x2808;
/// One past the array.
const SURFACE_PADS_END: usize = 0x2810;
/// The state's layer list.
const STATE_LAYERS: usize = 0x22e8;
/// The layer's sample.
const LAYER_SAMPLE: usize = 0x4a8;

/// What the trim needs from the glue.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// The app's control-surface setter.
    pub set_control: Option<unsafe extern "C" fn(usize, usize, i32, i32, f32) -> usize>,
    /// The app's repaint.
    pub repaint: Option<unsafe extern "C" fn(usize, u64, u64)>,
}

/// Everything the trim reads from the glue.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    /// The page's surface.
    pub surface: usize,
    /// The app's control context.
    pub control: usize,
    /// The component to repaint.
    pub component: usize,
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
///
/// # Safety
///
/// The context's pointers must be the app's live objects and the hooks its own
/// functions.
pub unsafe fn run(context: Context, hooks: Hooks) {
    let pad = if (0..128).contains(&context.last_pad) {
        context.last_pad
    } else {
        FALLBACK_PAD
    };
    let Some(sample) = sample_of(context.surface, pad) else {
        return;
    };
    // SAFETY: the sample is the page's own object.
    let frames = unsafe { sample::frames_of_sample(sample) };
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
    if let Some(set) = hooks.set_control {
        // SAFETY: the glue's own callback.
        unsafe {
            set(context.control, context.surface, pad, SET_START, 0.0);
            set(
                context.control,
                context.surface,
                pad,
                SET_END,
                last.saturating_add(1) as f32,
            );
            set(
                context.control,
                context.surface,
                pad,
                SET_START,
                first as f32,
            );
        }
    }
    if let (Some(repaint), true) = (hooks.repaint, context.component != 0) {
        // SAFETY: the glue's own callback.
        unsafe { repaint(context.component, 0, (2_000_u64 << 32) | 0x7d0) };
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
    // SAFETY: only the UI thread writes the surface.
    let surface = unsafe { *core::ptr::addr_of!(g_surface) };
    if surface == 0 {
        return sample::DEFAULT_ROOT;
    }
    sample_of(surface, pad).map_or(sample::DEFAULT_ROOT, sample::root_note)
}

/// The sample a pad holds, read through the surface's own tables.
fn sample_of(surface: usize, pad: i32) -> Option<usize> {
    let array = crate::firmware::read_pointer(surface + SURFACE_PADS)?;
    let end = crate::firmware::read_pointer(surface + SURFACE_PADS_END)?;
    if array == 0 || end < array {
        return None;
    }
    let index = usize::try_from(pad).ok()?;
    if (end - array) / 8 <= index {
        return None;
    }
    let state = crate::firmware::read_pointer(array + index * 8)?;
    if state == 0 {
        return None;
    }
    let layers = crate::firmware::read_pointer(state + STATE_LAYERS)?;
    if layers == 0 {
        return None;
    }
    let sample = crate::firmware::read_pointer(layers + LAYER_SAMPLE)?;
    (sample != 0).then_some(sample)
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

/// The app's control setter, as the page's knobs call it.
type SetControl = unsafe extern "C" fn(usize, usize, i32, i32, f32) -> usize;

/// The setter the mod displaced.
static mut ORIGINAL_SET: Option<SetControl> = None;
/// The control-surface object the app last handed to the setter.
#[unsafe(no_mangle)]
pub static mut g_surface: usize = 0;
/// The control context that came with it.
static mut CONTROL: usize = 0;

/// How far past the load address a vtable can point and still be the app's.
const IMAGE_SPAN: usize = 0x1200_0000;

/// Record the displaced setter, at install time.
///
/// # Safety
///
/// Called once, before the hook can run.
pub unsafe fn record_set_control(original: usize) {
    // SAFETY: the caller's contract; the address is the trampoline for a setter.
    unsafe {
        ORIGINAL_SET = (original != 0).then(|| core::mem::transmute::<usize, SetControl>(original));
    }
}

/// The app's setter, as a hook: learn the live surface, then pass the call on.
///
/// The page drives its trim through this entry; remembering the object it is
/// called with is what lets [`minimod_autotrim_cycle`] do the same.
///
/// # Safety
///
/// The app calls this with its own objects.
pub unsafe extern "C" fn minimod_hook_set_control(
    control: usize,
    surface: usize,
    id: i32,
    op: i32,
    value: f32,
) -> usize {
    let base = crate::firmware::base();
    let looks_live = surface != 0
        && crate::firmware::read_pointer(surface)
            .is_some_and(|vtable| (base..=base + IMAGE_SPAN).contains(&vtable));
    // SAFETY: only the UI thread calls the setter.
    unsafe {
        if looks_live {
            g_surface = surface;
            CONTROL = control;
        }
        // The setter tail-calls the app's own, so its result is its caller's.
        match ORIGINAL_SET {
            Some(original) => original(control, surface, id, op, value),
            None => 0,
        }
    }
}

/// Run one press of the trim cycle on the surface the app last used.
///
/// # Safety
///
/// Called on the UI thread, with the hook installed.
pub unsafe extern "C" fn minimod_autotrim_cycle() {
    // SAFETY: only the UI thread writes these.
    let (surface, control, set_control) = unsafe {
        (
            *core::ptr::addr_of!(g_surface),
            *core::ptr::addr_of!(CONTROL),
            *core::ptr::addr_of!(ORIGINAL_SET),
        )
    };
    let context = Context {
        surface,
        control,
        component: if crate::audio::painted() != 0 {
            crate::audio::painted()
        } else {
            paint::screen()
        },
        step: crate::ui::trim_step(),
        last_pad: crate::input::last_pad(),
    };
    let hooks = Hooks {
        set_control,
        repaint: crate::glue::original_repaint(),
    };
    // SAFETY: the context and hooks are the app's own.
    unsafe { run(context, hooks) };
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
