//! The menu's status line.
//!
//! Every sample action reports through one fixed buffer that the paint hook
//! draws and the acceptance tests read out of `/proc/<pid>/mem` by symbol.  The
//! buffer therefore stays a plain static with its name intact; only the
//! formatting moved.

use core::ffi::CStr;
use core::sync::atomic::{AtomicU8, Ordering};

/// Longest status line, including the terminating NUL.
pub const STATUS_BYTES: usize = 96;
/// Longest action name plus ": " plus the message.
const MESSAGE_BYTES: usize = STATUS_BYTES - 1;

/// The status the menu shows.  A byte array rather than a `String` so the
/// symbol the tests read is the storage itself; the export name is the one the
/// glue and the acceptance tests already use.
#[unsafe(export_name = "g_hpss_status")]
pub static mut STATUS: [u8; STATUS_BYTES] = [0; STATUS_BYTES];

/// Serialises writers so two threads cannot interleave half a message.
static LOCK: AtomicU8 = AtomicU8::new(0);

/// The action's name, as the menu shows it.
#[must_use]
pub const fn action_name(action: i32) -> &'static str {
    match action {
        1 => "Reverb",
        2 => "Vinyl",
        3 => "Lo-Fi",
        4 => "Bass Synth",
        _ => "HPSS",
    }
}

/// Replace the status line.
pub fn set_status(text: &str) {
    let mut bytes = [0_u8; STATUS_BYTES];
    let length = text.len().min(MESSAGE_BYTES);
    bytes[..length].copy_from_slice(&text.as_bytes()[..length]);
    write(&bytes);
}

/// Replace the status line with `"<action>: <message>"`.
pub fn set_action_status(action: i32, message: &str) {
    let name = action_name(action);
    let room = MESSAGE_BYTES.saturating_sub(name.len() + 2);
    let message = &message[..message.len().min(room)];
    let mut text = [0_u8; STATUS_BYTES];
    text[..name.len()].copy_from_slice(name.as_bytes());
    text[name.len()..name.len() + 2].copy_from_slice(b": ");
    let at = name.len() + 2;
    text[at..at + message.len()].copy_from_slice(message.as_bytes());
    write(&text);
}

/// Replace the buffer through a raw pointer, so no `&mut` to a static is ever
/// formed.
fn write(bytes: &[u8; STATUS_BYTES]) {
    lock();
    // SAFETY: `LOCK` is held, so no other writer touches the buffer, and the
    // copy is exactly its size.
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            core::ptr::addr_of_mut!(STATUS).cast::<u8>(),
            STATUS_BYTES,
        );
    }
    unlock();
}

fn lock() {
    while LOCK
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn unlock() {
    LOCK.store(0, Ordering::Release);
}

/// Borrow a NUL-terminated C string.
///
/// # Safety
///
/// `text` must be a valid NUL-terminated string, or null.
#[must_use]
pub unsafe fn c_text(text: *const core::ffi::c_char) -> &'static str {
    if text.is_null() {
        return "";
    }
    // SAFETY: the caller guarantees a NUL-terminated string.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    core::str::from_utf8(bytes).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::{STATUS_BYTES, action_name, set_action_status, set_status};

    fn status() -> String {
        // SAFETY: the tests are single-threaded over this buffer, and the copy
        // avoids forming a reference to it.
        let mut bytes = [0_u8; STATUS_BYTES];
        unsafe {
            core::ptr::copy_nonoverlapping(
                core::ptr::addr_of!(super::STATUS).cast::<u8>(),
                bytes.as_mut_ptr(),
                STATUS_BYTES,
            );
        }
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(0);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    #[test]
    fn action_names_match_the_menu() {
        assert_eq!(action_name(1), "Reverb");
        assert_eq!(action_name(4), "Bass Synth");
        assert_eq!(action_name(7), "HPSS");
    }

    #[test]
    fn a_status_is_prefixed_with_the_action() {
        set_action_status(4, "copy created");
        assert_eq!(status(), "Bass Synth: copy created");
        set_action_status(0, "no sample on this pad");
        assert_eq!(status(), "HPSS: no sample on this pad");
    }

    #[test]
    fn a_long_message_is_truncated_and_still_terminated() {
        let long = "x".repeat(STATUS_BYTES * 2);
        set_action_status(1, &long);
        let text = status();
        assert!(text.len() < STATUS_BYTES, "{}", text.len());
        assert!(text.starts_with("Reverb: xxx"));
        // Clearing works the same way.
        set_status("");
        assert_eq!(status(), "");
    }
}
