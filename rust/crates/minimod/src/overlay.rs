//! What the mod paints on top of the Sample page.
//!
//! The SDK lets the app draw first and then calls this; the overlay adds the
//! toast, the trim marks, the looper view, the effects menu and the `AutoTrim`
//! soft key.  The paint is also the safe point on the UI thread where a
//! finished job and the spectral chop advance.

use crate::paint;
use mpc_sample_sdk::draw::Graphics;

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
    let toast = paint::toast_text();
    let text = String::from_utf8_lossy(&toast);
    paint::text(graphics, &text, x + 4, y + 6, w - 8, h - 12, 18.0);
}

/// Draw the effects menu over the page.
fn menu(graphics: usize) {
    let state = crate::menu::state();
    crate::menu::draw_menu(
        graphics,
        state.selection,
        state.bass_edit,
        state.busy,
        &crate::status::text(),
        crate::menu::bass_knobs(),
    );
}

/// Finish a render the worker completed.
pub fn finalize() {
    if crate::jobflow::finish(paint::screen()) {
        crate::backend::bass::preview_set_active(false);
    }
}

/// Finish a completed job or a running spectral chop without waiting for the
/// Sample page to paint.
pub fn poll_offpage() {
    crate::spectral::cancel_if_page_gone();
    let screen = crate::jobflow::job_screen();
    if screen == 0 {
        return;
    }
    let current = paint::screen();
    // SAFETY: `current` is the component the paint hook last recorded on this
    // UI thread.
    let visible = unsafe { crate::page::visible(current) };
    if !visible || current != screen {
        finalize();
    }
}

/// The page's paint: draw the mod's overlay on top of the app's.
pub fn paint(graphics: Graphics, event: mpc_sample_sdk::PaintEvent) {
    if !event.sample_page {
        return;
    }
    let component = event.component;
    let handle = graphics.handle();
    crate::spectral::frame();
    let _ = crate::jobflow::finish(component);
    toast(handle);
    if crate::backend::looper::looper_is_open() != 0 {
        crate::looper_view::draw(handle);
        return;
    }
    if crate::menu::state().open != 0 {
        menu(handle);
        return;
    }
    if crate::ui::shift() == 0 || event.chop_mode || !crate::ui::trim_screen() {
        return;
    }
    paint::colour(handle, 0xff00_0000);
    paint::rect(handle, 213, 0, 107, 38);
    paint::colour(handle, 0xffff_ffff);
    paint::text(handle, "AutoTrim", 215, 5, 103, 28, 18.0);
}
