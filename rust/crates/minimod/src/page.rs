//! What the Sample page is currently showing.
//!
//! Two questions decide most of the mod's behaviour: is the Sample page the
//! visible screen, and is it in chop mode?  Both are answered by the page's own
//! predicate, not by guessing from state we track ourselves.

use crate::firmware;

/// The Sample page's vtable, as an offset from the image base.
pub const SAMPLE_PAGE_VTABLE: usize = 0x6e2_12a0;
/// `[screen + 0x1ee0]` is the chop-mode view the predicate takes.
const CHOP_VIEW: usize = 0x1ee0;
/// The app's predicate for "this view is in chop mode".
const CHOP_PREDICATE: usize = 0x112_fd50;

/// Whether `screen` is a Sample page.
fn is_sample_page(base: usize, screen: usize) -> bool {
    screen != 0
        && firmware::read_pointer(screen).is_some_and(|vtable| vtable == base + SAMPLE_PAGE_VTABLE)
}

/// Whether the Sample page is visible and showing a sample.
#[must_use]
pub fn visible(base: usize, screen: usize) -> bool {
    if !is_sample_page(base, screen) {
        return false;
    }
    // SAFETY: `base + CHOP_PREDICATE` is the page's own predicate.
    unsafe { firmware::int_of_pointer(base + CHOP_PREDICATE, screen) != 0 }
}

/// Whether the Sample page is in chop mode.
#[must_use]
pub fn chop_mode(base: usize, screen: usize) -> bool {
    if !is_sample_page(base, screen) {
        return false;
    }
    // SAFETY: same predicate, applied to the page's chop view.
    unsafe { firmware::int_of_pointer(base + CHOP_PREDICATE, screen.wrapping_add(CHOP_VIEW)) != 0 }
}

/// The Sample page's visibility, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_page_visible(base: usize, screen: usize) -> i32 {
    i32::from(visible(base, screen))
}

/// The Sample page's chop mode, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_page_chop_mode(base: usize, screen: usize) -> i32 {
    i32::from(chop_mode(base, screen))
}

#[cfg(test)]
mod tests {
    use super::{CHOP_VIEW, SAMPLE_PAGE_VTABLE, chop_mode, visible};

    #[test]
    fn a_null_screen_is_never_a_sample_page() {
        assert!(!visible(0x1000_0000, 0));
        assert!(!chop_mode(0x1000_0000, 0));
        assert_eq!(SAMPLE_PAGE_VTABLE, 0x6e2_12a0);
        assert_eq!(CHOP_VIEW, 0x1ee0);
    }
}
