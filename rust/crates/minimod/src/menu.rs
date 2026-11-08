//! The effects menu and the bass control page.
//!
//! The menu is a modal screen the mod draws itself: SHIFT + the encoder opens
//! it, the encoder picks a row, and a press runs it.  The state it keeps is the
//! open flag, the selected row and whether the bass control page is showing.

/// The rows of the effects menu.
/// The split.
pub const ROW_HPSS: i32 = 0;
/// The spectral chop.
pub const ROW_SPECTRAL: i32 = 1;
/// Reverb.
pub const ROW_REVERB: i32 = 2;
/// Vinyl.
pub const ROW_VINYL: i32 = 3;
/// Lo-Fi.
pub const ROW_LOFI: i32 = 4;
/// The bass synth.
pub const ROW_BASS: i32 = 5;
/// The live looper.
pub const ROW_LOOPER: i32 = 6;

/// The menu's state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// Whether the menu is showing.
    pub open: i32,
    /// Whether a job is running.
    pub busy: i32,
    /// Whether the bass control page is showing.
    pub bass_edit: i32,
    /// Whether the encoder went down already.
    pub data_down: i32,
    /// The selected row.
    pub selection: i32,
    /// Whether a chop restore is waiting for SHIFT to be released.
    pub restore_pending: i32,
}

/// What the glue has to supply.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Redraw the screen.
    pub repaint: Option<unsafe extern "C" fn()>,
    /// Render the split.
    pub start_split: Option<unsafe extern "C" fn()>,
    /// Render one effect, by its row.
    pub start_effect: Option<unsafe extern "C" fn(i32)>,
    /// Start the spectral chop.
    pub start_spectral: Option<unsafe extern "C" fn()>,
    /// Start the bass synth with its source backup.
    pub start_bass: Option<unsafe extern "C" fn()>,
    /// Put the chop mode back.
    pub restore_chop: Option<unsafe extern "C" fn()>,
    /// Show the bass control page.
    pub start_bass_page: Option<unsafe extern "C" fn()>,
    /// Open the live looper.
    pub start_looper: Option<unsafe extern "C" fn() -> i32>,
    /// Replace the status line.
    pub set_status: Option<unsafe extern "C" fn(*const u8)>,
}

/// Open the menu, if the page is showing and nothing is running.
#[must_use]
pub fn open(state: &mut State, hooks: Hooks, page_visible: bool) -> bool {
    if !page_visible || state.busy != 0 {
        return false;
    }
    state.open = 1;
    state.selection = 0;
    state.bass_edit = 0;
    state.data_down = 0;
    repaint(hooks);
    true
}

/// Close the menu unless a job is running.
pub fn close(state: &mut State, hooks: Hooks) -> bool {
    if state.busy != 0 {
        return false;
    }
    state.open = 0;
    state.bass_edit = 0;
    state.data_down = 0;
    repaint(hooks);
    true
}

/// Run the selected row.
///
/// # Safety
///
/// The hooks must be the glue's own callbacks.
pub unsafe fn activate(state: &mut State, hooks: Hooks) {
    if state.busy != 0 {
        return;
    }
    if state.bass_edit != 0 {
        if let Some(start) = hooks.start_bass {
            // SAFETY: the caller's callback.
            unsafe { start() };
        }
        return;
    }
    match state.selection {
        ROW_HPSS => {
            if let Some(start) = hooks.start_split {
                // SAFETY: the caller's callback.
                unsafe { start() };
            }
        }
        ROW_SPECTRAL => {
            if let Some(start) = hooks.start_spectral {
                // SAFETY: the caller's callback.
                unsafe { start() };
            }
        }
        ROW_REVERB | ROW_VINYL | ROW_LOFI => {
            if let Some(start) = hooks.start_effect {
                // SAFETY: the caller's callback.
                unsafe { start(state.selection - 1) };
            }
        }
        ROW_BASS => {
            if let Some(status) = hooks.set_status {
                // SAFETY: the message is a static, NUL-terminated string.
                unsafe { status(c"Live preview unavailable".as_ptr().cast()) };
            }
            if let Some(page) = hooks.start_bass_page {
                // SAFETY: the caller's callback.
                unsafe { page() };
            }
            repaint(hooks);
        }
        ROW_LOOPER => {
            if let Some(activate) = hooks.start_looper {
                // SAFETY: the caller's callback.
                let opened = unsafe { activate() };
                if opened != 0 {
                    state.open = 0;
                    state.bass_edit = 0;
                }
            }
            repaint(hooks);
        }
        _ => {
            close(state, hooks);
        }
    }
}

/// Put the chop mode back to what it was before the action.
///
/// # Safety
///
/// `restore` must be the glue's callback when present.
pub unsafe fn restore_chop(
    state: &mut State,
    restore: Option<unsafe extern "C" fn()>,
    allowed: bool,
) {
    if !allowed {
        state.restore_pending = 1;
        return;
    }
    if let Some(restore) = restore {
        // SAFETY: the caller's callback.
        unsafe { restore() };
    }
}

/// Redraw.
fn repaint(hooks: Hooks) {
    if let Some(repaint) = hooks.repaint {
        // SAFETY: the caller's callback.
        unsafe { repaint() };
    }
}

/// Open the menu, for the C glue.
///
/// # Safety
///
/// `state` and `hooks` must be live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_menu_open(
    state: *mut State,
    hooks: *const Hooks,
    page_visible: i32,
) -> i32 {
    if state.is_null() || hooks.is_null() {
        return 0;
    }
    // SAFETY: the caller promises both pointers are live.
    let (state, hooks) = unsafe { (&mut *state, *hooks) };
    i32::from(open(state, hooks, page_visible != 0))
}

/// Close the menu, for the C glue.
///
/// # Safety
///
/// `state` and `hooks` must be live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_menu_close(state: *mut State, hooks: *const Hooks) -> i32 {
    if state.is_null() || hooks.is_null() {
        return 0;
    }
    // SAFETY: the caller promises both pointers are live.
    let (state, hooks) = unsafe { (&mut *state, *hooks) };
    i32::from(close(state, hooks))
}

/// Run the selected row, for the C glue.
///
/// # Safety
///
/// `state` and `hooks` must be live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_menu_activate(state: *mut State, hooks: *const Hooks) {
    if state.is_null() || hooks.is_null() {
        return;
    }
    // SAFETY: the caller promises both pointers are live.
    let (state, hooks) = unsafe { (&mut *state, *hooks) };
    // SAFETY: the hooks are the caller's callbacks.
    unsafe { activate(state, hooks) };
}

/// Restore the chop mode, for the C glue.
///
/// # Safety
///
/// `state` and `hooks` must be live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_menu_restore_chop(
    state: *mut State,
    hooks: *const Hooks,
    allowed: i32,
) {
    if state.is_null() || hooks.is_null() {
        return;
    }
    // SAFETY: the caller promises both pointers are live.
    let (state, hooks) = unsafe { (&mut *state, *hooks) };
    // SAFETY: the hook is the caller's callback.
    unsafe { restore_chop(state, hooks.restore_chop, allowed != 0) };
}

#[cfg(test)]
mod tests {
    use super::{Hooks, ROW_HPSS, State, close, open};

    fn hooks() -> Hooks {
        Hooks {
            repaint: None,
            start_split: None,
            start_effect: None,
            start_spectral: None,
            start_bass: None,
            restore_chop: None,
            start_bass_page: None,
            start_looper: None,
            set_status: None,
        }
    }

    #[test]
    fn the_menu_opens_only_on_the_page_and_when_idle() {
        let mut state = State::default();
        assert!(open(&mut state, hooks(), true));
        assert_eq!(state.open, 1);
        assert_eq!(state.selection, ROW_HPSS);
        state.busy = 1;
        state.open = 0;
        assert!(!open(&mut state, hooks(), true), "busy");
        state.busy = 0;
        assert!(!open(&mut state, hooks(), false), "page not visible");
    }

    #[test]
    fn a_running_job_keeps_the_menu_open() {
        let mut state = State {
            open: 1,
            busy: 1,
            ..State::default()
        };
        assert!(!close(&mut state, hooks()));
        assert_eq!(state.open, 1);
        state.busy = 0;
        assert!(close(&mut state, hooks()));
        assert_eq!(state.open, 0);
    }
}
