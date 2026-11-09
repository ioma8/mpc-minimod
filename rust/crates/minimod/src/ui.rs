//! The keys the user is holding and what the page shows because of it.
//!
//! The button entry point keeps this state; the glue reads the same fields
//! through the module's accessor, so its own readers did not have to change.

use crate::buttons::{self, Action, Context, Held, Modals, OctaveKeys, Screens};
use crate::looper_ffi;
use crate::paint;

/// A full press, as the app reports it.
const PRESSED: u32 = 0x7f;
/// PLUS.
const KEY_PLUS: u32 = 0x36;
/// A pad press.
const KEY_PAD: u32 = 0x06;

/// The state the button entry point keeps.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct State {
    /// SHIFT, tracked while the looper owns the pads.
    pub shift: i32,
    /// ERASE, likewise.
    pub erase: i32,
    /// Which press of the trim cycle this is.
    pub trim_step: i32,
    /// Whether the trim key went down already.
    pub b3_down: i32,
    /// PLUS held.
    pub plus_held: i32,
    /// MINUS held.
    pub minus_held: i32,
    /// The 16 Levels octave shift.
    pub octave: i32,
    /// The first trim edge drawn last, or -1.
    pub mark_v0: f32,
    /// The second.
    pub mark_v1: f32,
    /// The object of the last pad press.
    pub last_press_obj: usize,
    /// The last button event, replayed to put the chop mode back.
    pub event: [u8; 16],
    /// Whether `event` holds a real press.
    pub event_valid: i32,
}

impl State {
    /// A fresh state: nothing held, no marks drawn.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            shift: 0,
            erase: 0,
            trim_step: 0,
            b3_down: 0,
            plus_held: 0,
            minus_held: 0,
            octave: 0,
            mark_v0: -1.0,
            mark_v1: -1.0,
            last_press_obj: 0,
            event: [0; 16],
            event_valid: 0,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// What the glue has to supply.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// The app's own button handler.
    pub forward: Option<unsafe extern "C" fn(usize)>,
    /// Toggle 16 Levels and keep the MIDI dispatcher in step.
    pub toggle_levels: Option<unsafe extern "C" fn()>,
    /// Open the effects menu.
    pub menu_open: Option<unsafe extern "C" fn()>,
    /// Close it.
    pub menu_close: Option<unsafe extern "C" fn()>,
    /// Run the selected row.
    pub menu_activate: Option<unsafe extern "C" fn()>,
    /// Rename the soft key.
    pub filter_text: Option<unsafe extern "C" fn(*const u8)>,
    /// Keep the MIDI dispatcher in step with the visible mode.
    pub midi_sync: Option<unsafe extern "C" fn(i32)>,
    /// Whether the 16 Levels screen is up.
    pub levels_active: Option<unsafe extern "C" fn() -> i32>,
    /// Whether the Sample page is visible.
    pub page_visible: Option<unsafe extern "C" fn() -> i32>,
    /// Whether the effects menu is open.
    pub menu_open_state: Option<unsafe extern "C" fn() -> i32>,
    /// Whether the bass control page is open.
    pub bass_edit_state: Option<unsafe extern "C" fn() -> i32>,
    /// Whether a job is running.
    pub busy_state: Option<unsafe extern "C" fn() -> i32>,
    /// Whether a spectral chop is running.
    pub spectral_state: Option<unsafe extern "C" fn() -> i32>,
    /// Put the chop mode back.
    pub restore_chop: Option<unsafe extern "C" fn()>,
    /// Redraw the menu.
    pub repaint: Option<unsafe extern "C" fn()>,
    /// Drop the trim edges the mod drew.
    pub clear_marks: Option<unsafe extern "C" fn()>,
    /// Move the 16 Levels row.
    pub bump_octave: Option<unsafe extern "C" fn(i32) -> i32>,
    /// Run one press of the trim cycle.
    pub run_autotrim: Option<unsafe extern "C" fn()>,
    /// Release every pad the app may think is held.
    pub release_held: Option<unsafe extern "C" fn()>,
    /// Read a pointer from the app's event.
    pub read_pointer: Option<unsafe extern "C" fn(usize) -> usize>,
}

/// One button event, handled.
///
/// # Safety
///
/// `event` must be the app's event object and `hooks` its own functions.
pub unsafe fn press(state: &mut State, event: usize, hooks: Hooks, data_down: &mut i32) -> i32 {
    let code = read_byte(event, 8);
    let down = read_byte(event, 9);
    let pressed = down == PRESSED;
    let levels = ask(hooks.levels_active);
    if let Some(sync) = hooks.midi_sync {
        // SAFETY: the glue's own callback.
        unsafe { sync(levels) };
    }
    let context = Context {
        code,
        down,
        held: Held {
            shift: state.shift != 0,
            erase: state.erase != 0,
            data: *data_down != 0,
        },
        octave: OctaveKeys {
            plus: state.plus_held != 0,
            minus: state.minus_held != 0,
        },
        screens: Screens {
            modals: Modals {
                menu: ask(hooks.menu_open_state) != 0,
                bass_edit: ask(hooks.bass_edit_state) != 0,
                spectral: ask(hooks.spectral_state) != 0,
            },
            looper: looper_ffi::looper_is_open() != 0,
            levels: levels != 0,
        },
        busy: ask(hooks.busy_state) != 0,
        page_visible: ask(hooks.page_visible) != 0,
    };
    let action = buttons::dispatch(context);
    if context.screens.looper {
        match action {
            Action::Shift(on) => state.shift = i32::from(on),
            Action::Erase(on) => state.erase = i32::from(on),
            Action::Looper => {
                let was = looper_ffi::looper_is_open() != 0;
                looper_ffi::looper_button(
                    code,
                    i32::try_from(down).unwrap_or(0),
                    state.shift,
                    state.erase,
                );
                if was && looper_ffi::looper_is_open() == 0 {
                    state.shift = 0;
                    state.erase = 0;
                    if let Some(release) = hooks.release_held {
                        // SAFETY: the glue's own callback.
                        unsafe { release() };
                    }
                    if let Some(repaint) = hooks.repaint {
                        // SAFETY: the glue's own callback.
                        unsafe { repaint() };
                    }
                }
            }
            _ => {}
        }
        return -2;
    }
    // Remember the event: the chop mode is put back by replaying it, and the
    // encoder's surface is reachable through the context it points at.
    // SAFETY: the app's event is a struct with the context pointer first.
    unsafe { remember(state, event, hooks) };
    // SAFETY: the hooks are the glue's callbacks.
    unsafe {
        apply(
            state,
            Event {
                address: event,
                code,
                pressed,
                action,
                context: &context,
                hooks,
                data_down,
            },
        );
    }
    kind(action)
}

/// The action as a number, for the glue's log.
const fn kind(action: Action) -> i32 {
    match action {
        Action::Forward => 0,
        Action::Consume => 1,
        Action::Shift(_) => 2,
        Action::Erase(_) => 3,
        Action::Looper | Action::LooperClosed => 4,
        Action::MenuOpen => 5,
        Action::MenuClose => 6,
        Action::MenuActivate => 7,
        Action::Autotrim => 8,
        Action::BumpOctave(_) => 9,
        Action::ToggleLevels => 10,
        Action::MenuEncoder => 11,
        Action::ResetPitch => 12,
        Action::RenameKey(_) => 13,
        Action::TrimCycleReset => 14,
        Action::RestoreChop => 15,
    }
}

/// One event, as the decision saw it.
struct Event<'a> {
    address: usize,
    code: u32,
    pressed: bool,
    action: Action,
    context: &'a Context,
    hooks: Hooks,
    data_down: &'a mut i32,
}

/// Apply one decided action.
unsafe fn apply(state: &mut State, event: Event<'_>) {
    let Event {
        address,
        code,
        pressed,
        action,
        context,
        hooks,
        data_down,
    } = event;
    let _ = context;
    match action {
        Action::MenuOpen | Action::MenuClose | Action::MenuActivate => {
            *data_down = i32::from(pressed);
            menu(action, hooks);
        }
        Action::MenuEncoder | Action::Consume => {
            if code == 0x6f {
                *data_down = i32::from(pressed);
            }
        }
        Action::Shift(on) => {
            state.shift = i32::from(on);
            if !pressed {
                state.trim_step = 0;
                state.b3_down = 0;
                if let Some(restore) = hooks.restore_chop {
                    // SAFETY: the glue's own callback, which decides for itself
                    // whether a restore is due.
                    unsafe { restore() };
                }
            }
            if let Some(text) = hooks.filter_text {
                let label = if pressed { c"AutoTrim" } else { c"Filter" };
                // SAFETY: the string is static.
                unsafe { text(label.as_ptr().cast()) };
            }
            forward(address, hooks);
        }
        Action::Autotrim => {
            if pressed && state.b3_down == 0 {
                if let Some(run) = hooks.run_autotrim {
                    // SAFETY: the glue's own callback.
                    unsafe { run() };
                }
                state.trim_step = (state.trim_step + 1) % 4;
            }
            state.b3_down = i32::from(pressed);
        }
        Action::BumpOctave(steps) => {
            if code == KEY_PLUS {
                state.plus_held = i32::from(pressed);
            } else {
                state.minus_held = i32::from(pressed);
            }
            if let Some(bump) = hooks.bump_octave {
                // SAFETY: the glue's own callback.
                unsafe { bump(steps) };
            }
        }
        Action::ResetPitch => {
            state.octave = 0;
            paint::show_toast("Pitch reset");
        }
        Action::ToggleLevels => {
            if let Some(toggle) = hooks.toggle_levels {
                // SAFETY: the glue's own callback owns the mode and the hook.
                unsafe { toggle() };
            }
        }
        Action::Forward => {
            if pressed && state.mark_v0 >= 0.0 {
                state.mark_v0 = -1.0;
                state.mark_v1 = -1.0;
                if let Some(clear) = hooks.clear_marks {
                    // SAFETY: the glue's own callback.
                    unsafe { clear() };
                }
            }
            // SAFETY: the app's event holds a pointer first.
            if code == KEY_PAD
                && pressed
                && let Some(object) = unsafe { read_object(address, hooks) }
            {
                state.last_press_obj = object;
            }
            forward(address, hooks);
        }
        Action::Erase(_)
        | Action::Looper
        | Action::LooperClosed
        | Action::RestoreChop
        | Action::TrimCycleReset
        | Action::RenameKey(_) => {}
    }
}

/// Run the selected menu row.
fn menu(action: Action, hooks: Hooks) {
    let callback = match action {
        Action::MenuOpen => hooks.menu_open,
        Action::MenuClose => hooks.menu_close,
        _ => hooks.menu_activate,
    };
    if let Some(callback) = callback {
        // SAFETY: the glue's own callback.
        unsafe { callback() };
    }
}

/// Store the event and derive what the encoder hook needs from it.
unsafe fn remember(state: &mut State, event: usize, hooks: Hooks) {
    if event == 0 {
        return;
    }
    let mut buffer = [0_u8; 16];
    // SAFETY: the app's event is at least this long.
    unsafe { read_replay(&mut buffer, event) };
    state.event = buffer;
    state.event_valid = 1;
    if let Some(read) = hooks.read_pointer {
        // SAFETY: the glue's own reader.
        let context = unsafe { read(event) };
        if context != 0 {
            // SAFETY: see above.
            let surface = unsafe { read(context + 0x10) };
            if surface != 0 {
                crate::input::set_data_target(surface + 0x2da8);
            }
        }
    }
}

/// The object the app's event points at, when it has one.
unsafe fn read_object(address: usize, hooks: Hooks) -> Option<usize> {
    let read = hooks.read_pointer?;
    // SAFETY: the app's event holds a pointer first.
    let object = unsafe { read(address) };
    (object != 0).then_some(object)
}

/// Forward the event to the app.
fn forward(event: usize, hooks: Hooks) {
    if let Some(forward) = hooks.forward {
        // SAFETY: the glue's own callback takes the app's own event.
        unsafe { forward(event) };
    }
}

/// Ask the glue a yes-or-no question.
fn ask(callback: Option<unsafe extern "C" fn() -> i32>) -> i32 {
    match callback {
        // SAFETY: the glue's own callback.
        Some(callback) => unsafe { callback() },
        None => 0,
    }
}

/// Read one byte of the event.
fn read_byte(event: usize, offset: usize) -> u32 {
    if event == 0 {
        return 0;
    }
    // SAFETY: the app's event is a struct with the key at these offsets.
    u32::from(unsafe { *(event as *const u8).add(offset) })
}

/// Copy the event's first sixteen bytes, for the chop restore.
unsafe fn read_replay(buffer: &mut [u8; 16], event: usize) {
    // SAFETY: the app's event is at least sixteen bytes.
    unsafe { core::ptr::copy_nonoverlapping(event as *const u8, buffer.as_mut_ptr(), 16) };
}

/// The 16 Levels octave shift.
#[must_use]
pub fn octave() -> i32 {
    // SAFETY: only the UI thread writes it.
    unsafe { (*core::ptr::addr_of!(STATE)).octave }
}

/// Move the 16 Levels octave shift.
pub fn set_octave(value: i32) {
    // SAFETY: only the UI thread writes it.
    unsafe { (*core::ptr::addr_of_mut!(STATE)).octave = value };
}

/// Which press of the trim cycle this is.
#[must_use]
pub fn trim_step() -> i32 {
    // SAFETY: only the UI thread writes it.
    unsafe { (*core::ptr::addr_of!(STATE)).trim_step }
}

/// Whether SHIFT is held.
#[must_use]
pub fn shift() -> i32 {
    // SAFETY: only the UI thread writes it.
    unsafe { (*core::ptr::addr_of!(STATE)).shift }
}

/// The state itself.
static mut STATE: State = State::new();

/// The menu's encoder-down flag, which the button hook tracks.
static mut DATA_DOWN: i32 = 0;

/// The UI state, for the glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_ui_state() -> *mut State {
    core::ptr::addr_of_mut!(STATE)
}

/// One button event, as the app's own hook.
///
/// # Safety
///
/// The app calls this with its own event object, and `hooks` must be the glue's
/// callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_buttons(event: usize, hooks: *const Hooks) -> i32 {
    if hooks.is_null() {
        return -1;
    }
    // SAFETY: the caller's contract.
    let hooks = unsafe { *hooks };
    // SAFETY: the state is a static in this module.
    let state = unsafe { &mut *core::ptr::addr_of_mut!(STATE) };
    // SAFETY: the flag is a static in this module.
    let data_down = unsafe { &mut *core::ptr::addr_of_mut!(DATA_DOWN) };
    // SAFETY: the hooks are the glue's callbacks.
    unsafe { press(state, event, hooks, data_down) }
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn a_fresh_state_holds_nothing_and_draws_no_marks() {
        let state = State::new();
        assert_eq!(state.shift, 0);
        assert_eq!(state.erase, 0);
        assert_eq!(state.trim_step, 0);
        assert!(state.mark_v0 < 0.0 && state.mark_v1 < 0.0);
        assert_eq!(state.event.len(), 16);
    }
}
