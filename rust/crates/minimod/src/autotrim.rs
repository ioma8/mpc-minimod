//! The `AutoTrim` soft key: trim the selected pad's recording to its audible
//! bounds, and clear the trim on the fourth press.
//!
//! The bounds come from [`crate::trim`], the file from the page's own path
//! helpers, and the result is applied through the app's control surface so the
//! page redraws its waveform exactly as if the user had moved the markers.

use crate::paint;
use crate::paths_ffi;
use crate::sample;
use crate::wav_ffi;

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
/// The longest path the app stores.
const PATH_LEN: usize = 512;

/// What the trim needs from the glue.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// The app's control-surface setter.
    pub set_control: Option<unsafe extern "C" fn(usize, usize, i32, i32, f32)>,
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
    // SAFETY: the path helpers take a writable buffer of PATH_LEN bytes.
    let Some(path) = path_of(sample, frames) else {
        return;
    };
    let Some(info) = info_of(&path) else {
        return;
    };
    if info.frames != frames {
        return;
    }
    let (first, last) = if context.step >= 3 {
        (0, frames.saturating_sub(1))
    } else {
        let ratio = RATIOS[usize::try_from(context.step).unwrap_or(0).min(2)];
        // SAFETY: the file is the one the app is showing.
        let Some((first, last)) = edges_of(&path, &info, ratio) else {
            return;
        };
        (first, last)
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

/// The path of the sample's WAV, trying the object, then the object graph, then
/// a scan by frame count.
fn path_of(sample: usize, frames: u32) -> Option<String> {
    let mut buffer = vec![0_u8; PATH_LEN];
    // SAFETY: the buffer is PATH_LEN bytes and the helpers NUL-terminate it.
    let found = unsafe {
        paths_ffi::path_from_object(sample, frames, buffer.as_mut_ptr().cast(), PATH_LEN) != 0
            || paths_ffi::wav_path_of(sample, frames, buffer.as_mut_ptr().cast(), PATH_LEN) != 0
            || paths_ffi::find_wav_by_frames(frames, buffer.as_mut_ptr().cast(), PATH_LEN) != 0
    };
    if !found {
        return None;
    }
    let end = buffer.iter().position(|&byte| byte == 0)?;
    if end == 0 {
        return None;
    }
    String::from_utf8(buffer[..end].to_vec()).ok()
}

/// One WAV's header facts.
struct Info {
    frames: u32,
    data_offset: u32,
    channels: u32,
    bytes: u32,
    format: u32,
}

/// Read a WAV's header.
fn info_of(path: &str) -> Option<Info> {
    let mut text = path.as_bytes().to_vec();
    text.push(0);
    let (mut frames, mut rate, mut channels) = (0_u32, 0_u32, 0_u32);
    let (mut offset, mut bytes, mut format) = (0_u32, 0_u32, 0_u32);
    // SAFETY: the path is NUL-terminated and the out-pointers are ours.
    let ok = unsafe {
        wav_ffi::wav_info(
            text.as_ptr().cast(),
            core::ptr::from_mut(&mut frames),
            core::ptr::from_mut(&mut rate),
            core::ptr::from_mut(&mut channels),
            core::ptr::from_mut(&mut offset),
            core::ptr::from_mut(&mut bytes),
            core::ptr::from_mut(&mut format),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(Info {
        frames,
        data_offset: offset,
        channels,
        bytes,
        format,
    })
}

/// The first and last audible frames at `ratio` of the file's peak.
fn edges_of(path: &str, info: &Info, ratio: f64) -> Option<(u32, u32)> {
    let mut text = path.as_bytes().to_vec();
    text.push(0);
    let (mut first, mut last) = (0_u32, 0_u32);
    // SAFETY: the path is NUL-terminated and the out-pointers are ours.
    let ok = unsafe {
        wav_ffi::wav_edges(
            text.as_ptr().cast(),
            info.frames,
            info.data_offset,
            info.channels,
            info.bytes,
            info.format,
            ratio,
            core::ptr::from_mut(&mut first),
            core::ptr::from_mut(&mut last),
        )
    };
    (ok != 0).then_some((first, last))
}

/// Run one press of the trim cycle, for the C glue.
///
/// # Safety
///
/// `context` and `hooks` must be readable and hold the app's live objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_autotrim_run(context: *const Context, hooks: *const Hooks) {
    if context.is_null() || hooks.is_null() {
        return;
    }
    // SAFETY: the caller's contract.
    let (context, hooks) = unsafe { (*context, *hooks) };
    // SAFETY: the hooks are the glue's own callbacks.
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
