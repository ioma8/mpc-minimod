//! What the mod paints on top of the Sample page.
//!
//! The page owns the screen: the mod hooks its paint, lets it draw, and then
//! adds its own overlays - the toast, the trim marks, the looper view, the
//! effects menu and the `AutoTrim` soft key.  It also uses this safe point on the
//! UI thread to run the work that must not run inside a button dispatch.

use crate::paint;

/// The Sample page's vtable, as an offset from the image base.
const SCREEN_VTABLE: usize = 0x6e2_12a0;

/// What the glue has to supply.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// The app's own paint.
    pub original: Option<unsafe extern "C" fn(usize, usize)>,
    /// Replay the queued knob command.
    pub commit_trim: Option<unsafe extern "C" fn()>,
    /// Advance the spectral chop.
    pub spectral_tick: Option<unsafe extern "C" fn()>,
    /// Finish a render that the worker completed; it guards itself.
    pub finalize: Option<unsafe extern "C" fn()>,
    /// Put the chop mode back.
    pub restore_chop: Option<unsafe extern "C" fn()>,
    /// Re-read the device rings.
    pub looper_pump: Option<unsafe extern "C" fn()>,
    /// Draw the effects menu.
    pub menu_draw: Option<unsafe extern "C" fn(usize)>,
}

/// The glue's flags this hook may clear.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Flags {
    /// A knob command is queued.
    pub pending_cc: i32,
    /// A chop restore is waiting.
    pub restore_pending: i32,
}

/// What one paint decided.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Outcome {
    /// The component was the Sample page.
    pub sample_page: i32,
    /// The screen pointer changed.
    pub screen_changed: i32,
}

/// Draw one frame.
///
/// # Safety
///
/// `component` and `graphics` must be the app's objects for this paint, `hooks`
/// must be the glue's callbacks, and `flags` must point at the glue's flags.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_overlay_paint(
    component: usize,
    graphics: usize,
    base: usize,
    hooks: *const Hooks,
    flags: *mut Flags,
    screen_pointer: *mut usize,
    shift: i32,
    chop_mode: i32,
    looper_open: i32,
    menu_open: i32,
    marks: *const f32,
) -> Outcome {
    let mut outcome = Outcome::default();
    if hooks.is_null() || flags.is_null() || screen_pointer.is_null() {
        return outcome;
    }
    // SAFETY: the caller promises both pointers are live for this call.
    let (hooks, flags) = unsafe { (*hooks, &mut *flags) };
    if let Some(original) = hooks.original {
        // SAFETY: the glue's own callback.
        unsafe { original(component, graphics) };
    }
    // A queued knob command is replayed here, at a safe point on the UI thread.
    if flags.pending_cc != 0 {
        flags.pending_cc = 0;
        if let Some(commit) = hooks.commit_trim {
            // SAFETY: the glue's own callback; it sets and clears its own
            // injection flag around the replay.
            unsafe { commit() };
        }
    }
    if !is_sample_page(base, component) {
        return outcome;
    }
    outcome.sample_page = 1;
    // SAFETY: the caller promises the pointer is live.
    let previous = unsafe { *screen_pointer };
    if previous != component {
        // SAFETY: see above.
        unsafe { *screen_pointer = component };
        outcome.screen_changed = 1;
    }
    if let Some(tick) = hooks.spectral_tick {
        // SAFETY: the glue's own callback.
        unsafe { tick() };
    }
    if let Some(finalize) = hooks.finalize {
        // SAFETY: the glue's own callback; it returns at once when nothing is
        // waiting to be finished.
        unsafe { finalize() };
    }
    if flags.restore_pending != 0 && shift == 0 {
        flags.restore_pending = 0;
        if let Some(restore) = hooks.restore_chop {
            // SAFETY: the glue's own callback.
            unsafe { restore() };
        }
    }
    toast(graphics);
    if !marks.is_null() {
        // SAFETY: the caller promises two readable values.
        let values = unsafe { core::slice::from_raw_parts(marks, 2) };
        trim_marks(graphics, values[0], values[1]);
    }
    if looper_open != 0 {
        if let Some(pump) = hooks.looper_pump {
            // SAFETY: the glue's own callback.
            unsafe { pump() };
        }
        // SAFETY: the graphics object is the app's.
        unsafe { crate::looper_view::minimod_looper_draw(graphics) };
        return outcome;
    }
    if menu_open != 0 {
        if let Some(menu) = hooks.menu_draw {
            // SAFETY: the glue's own callback.
            unsafe { menu(graphics) };
        }
        return outcome;
    }
    if shift == 0 || chop_mode != 0 {
        return outcome;
    }
    paint::colour(graphics, 0xff00_0000);
    paint::rect(graphics, 213, 0, 107, 38);
    paint::colour(graphics, 0xffff_ffff);
    paint::text(graphics, "AutoTrim", 215, 5, 103, 28, 18.0);
    outcome
}

/// Whether `component` is the Sample page.
fn is_sample_page(base: usize, component: usize) -> bool {
    component != 0
        && crate::firmware::read_pointer(component)
            .is_some_and(|vtable| vtable == base + SCREEN_VTABLE)
}

/// The toast box over the trim bar.
fn toast(graphics: usize) {
    if !paint::toast_visible(crate::clock::now_ms()) {
        return;
    }
    let (x, y, w, h) = (0, 194, 320, 46);
    paint::colour(graphics, 0xff15_151a);
    paint::rect(graphics, x, y, w, h);
    paint::colour(graphics, 0xffff_b43c);
    paint::rect(graphics, x, y, w, 2);
    paint::rect(graphics, x, y + h - 2, w, 2);
    paint::colour(graphics, 0xffff_ffff);
    let text = String::from_utf8_lossy(paint::toast_text());
    paint::text(graphics, &text, x + 4, y + 6, w - 8, h - 12, 18.0);
}

/// The trim edges the last autotrim produced, drawn where the page's markers
/// would be.
fn trim_marks(graphics: usize, first: f32, second: f32) {
    if first < 0.0 || second < 0.0 {
        return;
    }
    let x0 = 10 + (first * 295.0 + 0.5) as i32;
    let x1 = 10 + (second * 295.0 + 0.5) as i32;
    paint::colour(graphics, 0xff1b_1b1e);
    if x0 > 10 {
        paint::rect(graphics, 10, 88, x0 - 10, 98);
    }
    if x1 < 305 {
        paint::rect(graphics, x1, 88, 305 - x1, 98);
    }
    paint::colour(graphics, 0xffff_ffff);
    paint::rect(graphics, x0, 88, 1, 98);
    paint::rect(graphics, x1, 88, 1, 98);
    paint::rect(graphics, x0, 88, 3, 1);
    paint::rect(graphics, x1, 88, 3, 1);
}

/// Forget the chop machine when the hook is torn down.
pub fn reset() {
    crate::spectral_machine::reset();
}

/// Draw the effects menu over the page.
///
/// # Safety
///
/// `graphics` is the app's own.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_overlay_menu_draw(graphics: usize) {
    let menu = crate::menu::state();
    // SAFETY: the status line and the knobs are the crate's own.
    unsafe {
        crate::menu::minimod_menu_draw(
            graphics,
            menu.selection,
            menu.bass_edit,
            menu.busy,
            core::ptr::addr_of!(crate::status::STATUS).cast::<u8>(),
            core::ptr::addr_of!(crate::menu::g_bass_knob).cast::<f32>(),
        );
    }
}

/// Finish a render the worker completed, with the crate's own hooks.
///
/// # Safety
///
/// The crate's own state and the app's objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_overlay_finalize() {
    let hooks = crate::jobflow::glue_hooks();
    // SAFETY: the menu and the hooks are the crate's own.
    unsafe {
        crate::jobflow::minimod_glue_finish(
            crate::firmware::base(),
            paint::screen(),
            core::ptr::addr_of_mut!(crate::menu::g_menu),
            core::ptr::from_ref(&hooks),
        );
    }
}

/// The page's paint, with the mod's overlay on top.
///
/// # Safety
///
/// The app calls this with its own component and graphics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_overlay_paint_self(component: usize, graphics: usize) {
    let hooks = Hooks {
        original: crate::glue::minimod_paint_original(),
        commit_trim: None,
        spectral_tick: Some(minimod_spectral_tick_c),
        finalize: Some(minimod_overlay_finalize),
        restore_chop: Some(crate::ui::minimod_restore_chop),
        looper_pump: Some(crate::audio::minimod_pump_start),
        menu_draw: Some(minimod_overlay_menu_draw),
    };
    let mut flags = Flags {
        pending_cc: 0,
        restore_pending: crate::menu::state().restore_pending,
    };
    // SAFETY: the state is the UI module's.
    let state = unsafe { *crate::ui::minimod_ui_state() };
    let marks = [state.mark_v0, state.mark_v1];
    // SAFETY: the screen pointer and the flags are the crate's own.
    unsafe {
        minimod_overlay_paint(
            component,
            graphics,
            crate::firmware::base(),
            core::ptr::from_ref(&hooks),
            core::ptr::addr_of_mut!(flags),
            core::ptr::addr_of_mut!(paint::g_screen),
            crate::ui::shift(),
            crate::ui::minimod_screen_chop_mode(),
            crate::looper_ffi::looper_is_open(),
            crate::menu::state().open,
            marks.as_ptr(),
        );
    }
    crate::menu::state_mut().restore_pending = flags.restore_pending;
}

/// Advance the spectral chop from the paint.
unsafe extern "C" fn minimod_spectral_tick_c() {
    // SAFETY: the crate's own state and the app's screen.
    unsafe {
        crate::spectral::minimod_spectral_frame(crate::firmware::base(), paint::screen());
    }
}

#[cfg(test)]
mod tests {
    use super::{Flags, Hooks, Outcome};
    use core::mem::size_of;

    #[test]
    fn the_hooks_and_flags_have_stable_layouts() {
        assert_eq!(size_of::<Flags>(), 8);
        assert!(size_of::<Hooks>() >= 7 * size_of::<usize>());
        assert_eq!(size_of::<Outcome>(), 8);
    }
}
