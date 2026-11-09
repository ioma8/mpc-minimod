//! The effects menu and the bass control page.
//!
//! The menu is a modal screen the mod draws itself: SHIFT + the encoder opens
//! it, the encoder picks a row, and a press runs it.  The state it keeps is the
//! open flag, the selected row and whether the bass control page is showing.

use crate::paint;

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

/// Redraw.
fn repaint(hooks: Hooks) {
    if let Some(repaint) = hooks.repaint {
        // SAFETY: the caller's callback.
        unsafe { repaint() };
    }
}

/// Run the selected row, for the C glue.
///
/// # Safety
///
/// `state` and `hooks` must be live for this call.
pub unsafe extern "C" fn minimod_menu_activate(state: *mut State, hooks: *const Hooks) {
    if state.is_null() || hooks.is_null() {
        return;
    }
    // SAFETY: the caller promises both pointers are live.
    let (state, hooks) = unsafe { (&mut *state, *hooks) };
    // SAFETY: the hooks are the caller's callbacks.
    unsafe { activate(state, hooks) };
}

/// The menu's colours.
const PANEL: u32 = 0xff10_1014;
const ACCENT: u32 = 0xffff_b43c;
const WHITE: u32 = 0xffff_ffff;
const MUTED: u32 = 0xffb8_b8be;
const DIM: u32 = 0xff48_484f;
const SLOT: u32 = 0xff36_363d;
const ROW: u32 = 0xff3a_2a10;

/// The rows, in the order the encoder walks them.
const TITLES: [&str; MENU_ROWS] = [
    "HPSS",
    "Spectral Chop",
    "Add Reverb",
    "Vinyl",
    "Lo-Fi (Decimator)",
    "Bass Synth",
    "Live Looper",
    "Close",
];

/// The bass page's three knobs.
const KNOBS: [&str; 3] = ["BRIGHT", "EXCITE", "FILTER"];

/// Draw the menu.
///
/// `status` is the glue's status line (empty when there is nothing to say) and
/// `knobs` are the bass synth's three values.
#[must_use]
pub fn draw(graphics: usize, state: &State, busy: bool, status: &str, knobs: [f32; 3]) -> bool {
    let (x, y, w, h) = (14, 8, 292, 224);
    paint::colour(graphics, PANEL);
    paint::rect(graphics, x, y, w, h);
    paint::colour(graphics, ACCENT);
    paint::rect(graphics, x, y, w, 2);
    paint::rect(graphics, x, y + h - 2, w, 2);
    if state.bass_edit == 0 {
        paint::colour(graphics, WHITE);
        paint::text(graphics, "SAMPLE OPTIONS", x + 10, y + 6, w - 20, 22, 18.0);
    }
    if busy {
        paint::text(graphics, "Processing...", x + 10, y + 70, w - 20, 24, 16.0);
        if !status.is_empty() {
            paint::text(graphics, status, x + 10, y + 98, w - 20, 20, 12.0);
        }
        return false;
    }
    if state.bass_edit != 0 {
        draw_knobs(graphics, (x, y, w), status, knobs);
        return false;
    }
    for (index, title) in TITLES.iter().enumerate() {
        let row = y + 26 + i32::try_from(index).unwrap_or(0) * 20;
        if state.selection == i32::try_from(index).unwrap_or(-1) {
            paint::colour(graphics, ROW);
            paint::rect(graphics, x + 6, row, w - 12, 19);
            paint::colour(graphics, WHITE);
        }
        paint::text(graphics, title, x + 12, row + 1, w - 24, 17, 12.0);
    }
    if !status.is_empty() {
        paint::text(graphics, status, x + 10, y + 200, w - 20, 14, 10.0);
    }
    paint::colour(graphics, 0xffb0_b0b0);
    paint::text(
        graphics,
        "TURN: select    DATA: confirm",
        x + 10,
        y + h - 17,
        w - 20,
        14,
        10.0,
    );
    true
}

/// The bass control page.
fn draw_knobs(graphics: usize, area: (i32, i32, i32), status: &str, knobs: [f32; 3]) {
    let (x, y, w) = area;
    paint::colour(graphics, ACCENT);
    paint::text(graphics, "BASS SYNTH", x + 12, y + 6, w - 24, 22, 18.0);
    paint::colour(graphics, MUTED);
    paint::text(
        graphics,
        "PADS C2-D#3  /  CHROMATIC",
        x + 12,
        y + 36,
        w - 24,
        16,
        11.0,
    );
    let columns = [x + 16, x + 108, x + 200];
    for (index, column) in columns.iter().enumerate() {
        paint::colour(graphics, ACCENT);
        paint::text(
            graphics,
            &format!("K{}", index + 1),
            *column,
            y + 72,
            76,
            16,
            11.0,
        );
        paint::colour(graphics, WHITE);
        paint::text(graphics, KNOBS[index], *column, y + 90, 76, 18, 12.0);
        paint::colour(graphics, SLOT);
        paint::rect(graphics, *column, y + 116, 76, 4);
        paint::colour(graphics, ACCENT);
        let filled = (76.0 * knobs[index].clamp(0.0, 1.0) + 0.5) as i32;
        paint::rect(graphics, *column, y + 116, filled, 4);
        paint::colour(graphics, WHITE);
        paint::text(
            graphics,
            &format!("{:.2}", knobs[index]),
            *column,
            y + 128,
            76,
            20,
            14.0,
        );
    }
    paint::colour(graphics, DIM);
    paint::rect(graphics, x + 12, y + 164, w - 24, 1);
    if !status.is_empty() {
        paint::colour(graphics, 0xffe8_e8ec);
        paint::text(graphics, status, x + 12, y + 168, w - 24, 14, 10.0);
    }
    paint::colour(graphics, ACCENT);
    paint::text(
        graphics,
        "PADS: PLAY   DATA: SAVE   SHIFT+DATA: BACK",
        x + 10,
        y + 192,
        w - 20,
        16,
        10.0,
    );
}

/// The number of rows.
pub const MENU_ROWS: usize = 8;

/// Draw the menu, for the C glue.
///
/// # Safety
///
/// `status` must be a NUL-terminated string and `graphics` the app's object.
pub unsafe extern "C" fn minimod_menu_draw(
    graphics: usize,
    selection: i32,
    bass_edit: i32,
    busy: i32,
    status: *const u8,
    knobs: *const f32,
) {
    if status.is_null() || knobs.is_null() {
        return;
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let text = unsafe { core::ffi::CStr::from_ptr(status.cast()) };
    let status = text.to_str().unwrap_or("");
    // SAFETY: the caller promises three readable knob values.
    let values = unsafe { core::slice::from_raw_parts(knobs, 3) };
    let state = State {
        open: 1,
        busy,
        bass_edit,
        data_down: 0,
        selection,
        restore_pending: 0,
    };
    let _ = draw(
        graphics,
        &state,
        busy != 0,
        status,
        [values[0], values[1], values[2]],
    );
}

/// The menu's state.  It keeps its name because the glue reads its fields.
#[unsafe(no_mangle)]
pub static mut g_menu: State = State {
    open: 0,
    busy: 0,
    bass_edit: 0,
    data_down: 0,
    selection: 0,
    restore_pending: 0,
};

/// The menu's state, for the crate.
#[must_use]
pub fn state() -> &'static State {
    // SAFETY: only the UI thread writes it.
    unsafe { &*core::ptr::addr_of!(g_menu) }
}

/// The menu's state, to change.
pub fn state_mut() -> &'static mut State {
    // SAFETY: only the UI thread writes it.
    unsafe { &mut *core::ptr::addr_of_mut!(g_menu) }
}

/// The bass control page's three knobs.
#[unsafe(no_mangle)]
pub static mut g_bass_knob: [f32; 3] = [0.52, 0.18, 0.56];

/// Show the bass control page and start its preview.
///
/// # Safety
///
/// The app's own calls are used.
pub unsafe extern "C" fn minimod_menu_bass_page() {
    // SAFETY: the array is this module's.
    let knobs = unsafe { *core::ptr::addr_of!(g_bass_knob) };
    let menu = state_mut();
    menu.bass_edit = 1;
    crate::bass_ffi::bass_preview_set_params(knobs[0], knobs[1], knobs[2]);
    crate::bass_ffi::minimod_bass_preview_set_active(1);
    // SAFETY: the flag is the bass module's.
    let available = unsafe { crate::bass_ffi::G_BASS_PREVIEW_AVAILABLE } != 0;
    crate::status::set_status(if available {
        ""
    } else {
        "Live preview unavailable"
    });
}

/// Open the effects menu, if the page is showing and nothing is running.
///
/// # Safety
///
/// The crate's own state and the app's repaint.
pub unsafe extern "C" fn minimod_menu_open_self() {
    if crate::ui::minimod_screen_visible() == 0 || state().busy != 0 {
        return;
    }
    let menu = state_mut();
    menu.open = 1;
    menu.selection = 0;
    menu.bass_edit = 0;
    menu.data_down = 0;
    crate::bass_ffi::minimod_bass_preview_set_active(0);
    crate::status::set_status("");
    // SAFETY: the app's own repaint.
    unsafe { crate::ui::minimod_repaint_screen() };
}

/// Close the menu, unless a job is running.
///
/// # Safety
///
/// The crate's own state and the app's repaint.
pub unsafe extern "C" fn minimod_menu_close_self() {
    if state().busy != 0 {
        return;
    }
    let menu = state_mut();
    menu.open = 0;
    menu.bass_edit = 0;
    menu.data_down = 0;
    crate::bass_ffi::minimod_bass_preview_set_active(0);
    // SAFETY: the app's own repaint.
    unsafe { crate::ui::minimod_repaint_screen() };
}

/// Rename the soft key, which the app draws itself.
///
/// # Safety
///
/// The text is ignored; the rename happens on the paint.
pub unsafe extern "C" fn minimod_filter_text(_text: *const u8) {
    if paint::screen() != 0 {
        // SAFETY: the app's own repaint.
        unsafe { crate::ui::minimod_repaint_screen() };
    }
}

/// Whether the menu is open.
#[must_use]
pub extern "C" fn minimod_menu_is_open() -> i32 {
    i32::from(state().open != 0)
}

/// Whether the bass control page is showing.
#[must_use]
pub extern "C" fn minimod_menu_is_bass_edit() -> i32 {
    i32::from(state().bass_edit != 0)
}

/// Whether a job is running.
#[must_use]
pub extern "C" fn minimod_menu_is_busy() -> i32 {
    i32::from(state().busy != 0)
}

#[cfg(test)]
mod tests {
    use super::{Hooks, State, close};

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
