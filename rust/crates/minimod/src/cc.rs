//! What a control-change event does before it reaches the app.
//!
//! The routing decision is [`crate::routing`]'s; this acts on it: the master
//! volume and the latency trim belong to the looper, the three knobs belong to
//! the bass page, and everything else is either swallowed (a modal screen owns
//! the controls) or handed to the app.

use crate::bass_ffi;
use crate::clock;
use crate::looper_ffi;
use crate::routing;

/// How often the settings file may be written, in milliseconds.
const SAVE_EVERY_MS: i64 = 5_000;

/// The encoder's control number; only it reaches the app past a modal screen.
const DATA: u32 = 0x64;

/// What the glue has to supply.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// The app's own control-change handler.
    pub original: Option<unsafe extern "C" fn(usize, u32, f32, f32)>,
    /// Redraw the menu.
    pub repaint: Option<unsafe extern "C" fn()>,
    /// Write the settings file.
    pub save: Option<unsafe extern "C" fn()>,
    /// Show a one-line notice.
    pub notice: Option<unsafe extern "C" fn(*const u8)>,
}

/// The state the routing reads.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    /// Whether the mod is replaying its own command.
    pub replaying: i32,
    /// The raw value.
    pub value: u32,
    /// The controller's range.
    pub range: f32,
    /// The looper view.
    pub looper_open: i32,
    /// The effects menu.
    pub menu_open: i32,
    /// The bass control page.
    pub bass_edit: i32,
    /// A job is running.
    pub busy: i32,
    /// A spectral chop is running.
    pub spectral_active: i32,
    /// The latency trim in frames.
    pub trim_frames: i32,
    /// The three bass knob values.
    pub knobs: [f32; 3],
}

/// What happened, for the glue to apply.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Outcome {
    /// 1 when the event must not reach the app.
    pub consumed: i32,
    /// The new latency trim, in frames.
    pub trim_frames: i32,
    /// Whether `trim_frames` changed.
    pub trim_changed: i32,
    /// The three knob values after this event.
    pub knobs: [f32; 3],
    /// Whether `knobs` changed.
    pub knobs_changed: i32,
}

/// When the settings were last written.
static mut SAVED_AT: i64 = 0;

/// Handle one control-change event.
///
/// # Safety
///
/// `hooks` must be the glue's callbacks.
#[must_use]
pub unsafe fn handle(context: Context, control: u32, hooks: Hooks) -> Outcome {
    let mut outcome = Outcome {
        consumed: 0,
        trim_frames: context.trim_frames,
        trim_changed: 0,
        knobs: context.knobs,
        knobs_changed: 0,
    };
    let route = routing::minimod_cc_route(
        context.replaying,
        control,
        context.value,
        context.range,
        context.looper_open,
        context.menu_open,
        context.bass_edit,
        context.busy,
        context.spectral_active,
        context.trim_frames,
    );
    match route.kind {
        // The master volume drives the loops, then the app's own mix.
        1 => {
            looper_ffi::looper_set_master(context.value);
            save_throttled(hooks);
        }
        // The latency trim moves one millisecond per step.
        2 => {
            outcome.trim_frames = route.value;
            outcome.trim_changed = i32::from(route.value != context.trim_frames);
            save_throttled(hooks);
            // SAFETY: the glue's own callback.
            if let Some(notice) = hooks.notice {
                let millis = route.value / 44;
                let line = if millis >= 0 {
                    format!("TRIM +{millis} ms")
                } else {
                    format!("TRIM {millis} ms")
                };
                let mut text = line.into_bytes();
                text.push(0);
                // SAFETY: the text lives for the call and is NUL-terminated.
                unsafe { notice(text.as_ptr()) };
            }
            outcome.consumed = 1;
            return outcome;
        }
        // The bass control page owns the three knobs.
        3 => {
            let index = usize::try_from(route.value).unwrap_or(0).min(2);
            let mut knobs = context.knobs;
            knobs[index] = route.amount;
            outcome.knobs = knobs;
            outcome.knobs_changed = 1;
            bass_ffi::bass_preview_set_params(knobs[0], knobs[1], knobs[2]);
            if let Some(repaint) = hooks.repaint {
                // SAFETY: the glue's own callback.
                unsafe { repaint() };
            }
            outcome.consumed = 1;
            return outcome;
        }
        // A modal screen keeps the app away from the sample.
        4 => {
            outcome.consumed = 1;
            return outcome;
        }
        _ => {}
    }
    // Only DATA reaches the app while a modal screen owns the controls; the knobs
    // must not edit the sample behind it.
    if context.replaying == 0
        && (context.menu_open != 0 || context.spectral_active != 0)
        && control != DATA
    {
        outcome.consumed = 1;
        return outcome;
    }
    outcome
}

/// Write the settings, at most once every few seconds.
fn save_throttled(hooks: Hooks) {
    let now = clock::now_ms();
    // SAFETY: only the UI thread calls this.
    let saved = unsafe { *core::ptr::addr_of!(SAVED_AT) };
    if now <= saved + SAVE_EVERY_MS {
        return;
    }
    // SAFETY: see above.
    unsafe { *core::ptr::addr_of_mut!(SAVED_AT) = now };
    if let Some(save) = hooks.save {
        // SAFETY: the glue's own callback.
        unsafe { save() };
    }
}

/// Handle one control change, for the C glue.
///
/// # Safety
///
/// `hooks` must be readable and hold the glue's callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_cc_handle(
    context: *const Context,
    control: u32,
    hooks: *const Hooks,
) -> Outcome {
    if context.is_null() || hooks.is_null() {
        return Outcome::default();
    }
    // SAFETY: the caller's contract.
    let (context, hooks) = unsafe { (*context, *hooks) };
    // SAFETY: the hooks are the glue's callbacks.
    unsafe { handle(context, control, hooks) }
}

// The glue owns the screens, the key flags and the trim value; it builds the
// context and applies what comes back.
unsafe extern "C" {
    fn minimod_glue_cc_context() -> Context;
    fn minimod_glue_set_trim(frames: i32);
    fn minimod_glue_set_knobs(knobs: *const f32);
    fn minimod_glue_cc_notice(text: *const u8);
    fn minimod_glue_save();
}

/// The app's own control-change handler, recorded at install time.
static mut ORIGINAL: Option<unsafe extern "C" fn(usize, u32, f32, f32)> = None;

/// Record the displaced handler, for the glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_hook_cc_original(
    original: Option<unsafe extern "C" fn(usize, u32, f32, f32)>,
) {
    // SAFETY: the constructor calls this once, before any event can arrive.
    unsafe { ORIGINAL = original };
}

/// A control change, as the app's own hook.
///
/// # Safety
///
/// The app calls this with its own event pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_cc(event: usize, raw: u32, low: f32, range: f32) {
    // SAFETY: the glue builds the context from state it owns.
    let mut context = unsafe { minimod_glue_cc_context() };
    // The value and the range belong to the event, not to the glue.
    context.value = raw;
    context.range = range;
    let control = if event == 0 {
        0xffff
    } else {
        // SAFETY: the app's event is a struct whose control byte is at 8.
        u32::from(unsafe { *(event as *const u8).add(8) })
    };
    let hooks = Hooks {
        original: None,
        repaint: Some(crate::ui::minimod_repaint_screen),
        save: Some(minimod_glue_save),
        notice: Some(minimod_glue_cc_notice),
    };
    // SAFETY: the hooks are the glue's callbacks.
    let outcome = unsafe { handle(context, control, hooks) };
    if outcome.trim_changed != 0 {
        // SAFETY: the glue owns the value.
        unsafe { minimod_glue_set_trim(outcome.trim_frames) };
    }
    if outcome.knobs_changed != 0 {
        // SAFETY: the glue owns the three values.
        unsafe { minimod_glue_set_knobs(outcome.knobs.as_ptr()) };
    }
    if outcome.consumed != 0 {
        return;
    }
    // SAFETY: the original is the app's own handler.
    if let Some(original) = unsafe { *core::ptr::addr_of!(ORIGINAL) } {
        // SAFETY: the caller passed the app's event.
        unsafe { original(event, raw, low, range) };
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, Hooks, handle};

    fn context() -> Context {
        Context {
            replaying: 0,
            value: 64,
            range: 127.0,
            looper_open: 0,
            menu_open: 0,
            bass_edit: 0,
            busy: 0,
            spectral_active: 0,
            trim_frames: 0,
            knobs: [0.5, 0.5, 0.5],
        }
    }

    fn hooks() -> Hooks {
        Hooks {
            original: None,
            repaint: None,
            save: None,
            notice: None,
        }
    }

    #[test]
    fn an_ordinary_knob_reaches_the_app() {
        // SAFETY: no callbacks are set, so nothing is invoked.
        let outcome = unsafe { handle(context(), 0x20, hooks()) };
        assert_eq!(outcome.consumed, 0);
    }

    #[test]
    fn a_modal_screen_swallows_the_knobs_but_not_data() {
        let modal = Context {
            menu_open: 1,
            ..context()
        };
        // SAFETY: no callbacks are set.
        let outcome = unsafe { handle(modal, 0x20, hooks()) };
        assert_eq!(outcome.consumed, 1);
        // SAFETY: no callbacks are set.
        let data = unsafe { handle(modal, 0x64, hooks()) };
        assert_eq!(data.consumed, 0);
    }
}
