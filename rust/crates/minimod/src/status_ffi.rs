//! C ABI of the status line.

use core::ffi::c_char;

use crate::status;

/// Replace the status line.
///
/// # Safety
///
/// `text` must be a valid NUL-terminated string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hpss_set_status(text: *const c_char) {
    // SAFETY: see the function contract.
    status::set_status(unsafe { status::c_text(text) });
}

/// Replace the status line with `"<action>: <message>"`.
///
/// # Safety
///
/// `message` must be a valid NUL-terminated string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sample_action_status(action: i32, message: *const c_char) {
    // SAFETY: see the function contract.
    status::set_action_status(action, unsafe { status::c_text(message) });
}

/// The action's name as a C string.
#[unsafe(no_mangle)]
pub extern "C" fn sample_action_name(action: i32) -> *const c_char {
    match action {
        1 => c"Reverb".as_ptr(),
        2 => c"Vinyl".as_ptr(),
        3 => c"Lo-Fi".as_ptr(),
        4 => c"Bass Synth".as_ptr(),
        _ => c"HPSS".as_ptr(),
    }
}
