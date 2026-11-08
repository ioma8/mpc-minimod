//! Drawing on the app's own Graphics object.
//!
//! Ported but **not wired**: the glue still draws with its own calls.  Wiring it
//! needs `g_screen`, `orig_repaint` and the paint hook's forward declarations
//! moved in one edit rather than several.
//!
//! The mod does not own the screen: it hooks the page's paint and draws on top
//! with the app's own calls (setColour, fillRect, Font + drawText).  Keeping the
//! call sites in one place means the offsets and argument shapes live once.

use crate::firmware::{self, TextBox};

/// `setColour(uint32_t)`.
const SET_COLOUR: usize = 0x102_d5e0;
/// `fillRect(int, int, int, int)`.
const FILL_RECT: usize = 0x102_e000;
/// `String(const char *)`.
const STRING_FROM_C: usize = 0x0f5_35c0;
/// `Font(int, float)`.
const FONT: usize = 0x108_2810;
/// `setFont(const Font &)`.
const SET_FONT: usize = 0x102_dca0;
/// `drawText(const String &, int, int, int, int, int, int, float)`.
const DRAW_TEXT: usize = 0x108_17b0;
/// `Font::~Font()`.
const FONT_DROP: usize = 0x107_0d60;
/// `String::~String()`.
const STRING_DROP: usize = 0x0f0_a290;

/// Set the fill colour.
pub fn colour(graphics: usize, value: u32) {
    // SAFETY: `graphics` is the app's object and the offset its own call.
    unsafe { firmware::void_of_colour(firmware::base() + SET_COLOUR, graphics, value) };
}

/// Fill a rectangle in the current colour.
pub fn rect(graphics: usize, x: i32, y: i32, width: i32, height: i32) {
    // SAFETY: as above.
    unsafe { firmware::void_of_five(firmware::base() + FILL_RECT, graphics, x, y, width, height) };
}

/// Draw a line of text in the current colour.
pub fn text(graphics: usize, line: &str, x: i32, y: i32, width: i32, height: i32, size: f32) {
    let mut bytes = [0_u8; 96];
    let take = line.len().min(bytes.len() - 1);
    bytes[..take].copy_from_slice(&line.as_bytes()[..take]);
    let base = firmware::base();
    let mut string = [0_u64; 2];
    let mut font = [0_u64; 2];
    // SAFETY: the string is NUL-terminated and both objects are live for the
    // calls below, which is how the app builds them.
    unsafe {
        firmware::string_from_c(
            base + STRING_FROM_C,
            string.as_mut_ptr() as usize,
            bytes.as_ptr() as usize,
        );
        let font_fn: unsafe extern "C" fn(usize, i32, f32) = core::mem::transmute(base + FONT);
        font_fn(font.as_mut_ptr() as usize, 0, size);
        let set_font: unsafe extern "C" fn(usize, usize) = core::mem::transmute(base + SET_FONT);
        set_font(graphics, font.as_ptr() as usize);
        firmware::void_of_eight(
            base + DRAW_TEXT,
            graphics,
            string.as_ptr() as usize,
            TextBox {
                x,
                y,
                width,
                height,
                flags: 0x24,
                align: 1,
                opacity: 0.7,
            },
        );
        let drop_font: unsafe extern "C" fn(usize) = core::mem::transmute(base + FONT_DROP);
        drop_font(font.as_mut_ptr() as usize);
        let drop_string: unsafe extern "C" fn(usize) = core::mem::transmute(base + STRING_DROP);
        drop_string(string.as_mut_ptr() as usize);
    }
}

/// The toast: text plus a deadline, drawn by the paint hook so it costs nothing
/// when idle.
#[derive(Clone, Copy)]
struct Toast {
    until: i64,
    text: [u8; 48],
}

static mut TOAST: Toast = Toast {
    until: 0,
    text: [0; 48],
};

/// The app's repaint: component and two packed rectangles.
pub type Repaint = unsafe extern "C" fn(usize, u64, u64);

/// Show a toast and keep the screen repainting until it expires.
///
/// # Safety
///
/// `repaint` must be the app's repaint and `component` a live component.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_toast(
    line: *const u8,
    repaint: Option<Repaint>,
    component: usize,
) {
    if line.is_null() {
        return;
    }
    let now = crate::clock::now_ms();
    // SAFETY: only the UI thread shows toasts.
    let toast = unsafe { &mut *core::ptr::addr_of_mut!(TOAST) };
    let mut length = 0;
    // SAFETY: the caller passes a NUL-terminated string.
    unsafe {
        while length < toast.text.len() - 1 && *line.add(length) != 0 {
            length += 1;
        }
        for (slot, byte) in toast
            .text
            .iter_mut()
            .zip(core::slice::from_raw_parts(line, length))
        {
            *slot = *byte;
        }
        toast.text[length] = 0;
    }
    toast.until = now + 1000;
    redraw(repaint, component);
    std::thread::spawn(move || {
        std::thread::sleep(core::time::Duration::from_millis(1120));
        // SAFETY: the app's repaint is safe to call from this thread, which is
        // what the glue's own expiry thread did.
        redraw(repaint, component);
    });
}

/// Ask the app to repaint the screen.
fn redraw(repaint: Option<Repaint>, component: usize) {
    if let Some(repaint) = repaint
        && component != 0
    {
        // SAFETY: the caller guarantees this is the app's repaint.
        unsafe { repaint(component, 0, (2000_u64 << 32) | 0x7d0) };
    }
}

/// Show a toast for the standard one second.
pub fn show_toast(text: &str) {
    // SAFETY: the text is valid for the call, and the module owns the buffer it
    // is copied into.
    unsafe { minimod_toast(text.as_ptr(), None, 0) };
}

/// The toast's text, or an empty string.
#[must_use]
pub fn toast_text() -> &'static [u8] {
    // SAFETY: only the UI thread reads this, and the buffer lives for the
    // process; the caller stops at the NUL.
    let toast = unsafe { &*core::ptr::addr_of!(TOAST) };
    let end = toast.text.iter().position(|&byte| byte == 0).unwrap_or(0);
    &toast.text[..end]
}

/// Whether a toast is still on screen.
#[must_use]
pub fn toast_visible(now: i64) -> bool {
    // SAFETY: only the UI thread reads this.
    let toast = unsafe { *core::ptr::addr_of!(TOAST) };
    toast.until > now && toast.text[0] != 0
}

/// The toast's text, for the paint hook.
///
/// # Safety
///
/// The returned pointer is valid until the next toast.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_toast_text() -> *const u8 {
    // SAFETY: only the UI thread reads this.
    unsafe { (*core::ptr::addr_of!(TOAST)).text.as_ptr() }
}

/// Fill a rectangle, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_paint_colour(graphics: usize, value: u32) {
    colour(graphics, value);
}

/// Fill a rectangle, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_paint_rect(graphics: usize, x: i32, y: i32, width: i32, height: i32) {
    rect(graphics, x, y, width, height);
}

/// Draw text, for the C glue.
///
/// # Safety
///
/// `line` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_paint_text(
    graphics: usize,
    line: *const u8,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    size: f32,
) {
    if line.is_null() {
        return;
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let bytes = unsafe { core::ffi::CStr::from_ptr(line.cast()) };
    text(
        graphics,
        bytes.to_str().unwrap_or(""),
        x,
        y,
        width,
        height,
        size,
    );
}

/// Whether a toast is on screen, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_toast_visible(now: i64) -> i32 {
    i32::from(toast_visible(now))
}

#[cfg(test)]
mod tests {
    use super::toast_visible;

    #[test]
    fn nothing_is_drawn_before_a_toast() {
        assert!(!toast_visible(0));
    }
}
