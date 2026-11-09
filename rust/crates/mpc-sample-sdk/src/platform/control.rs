//! The app's control-surface setter, recorded and replayed.
//!
//! The Sample page drives its trim through this entry and passes it the live
//! surface object.  The SDK remembers that object and the displaced setter, so a
//! mod can apply a control change exactly as the page would.

use crate::image;

/// The app's control setter, as the page's knobs call it.
pub type SetControl = unsafe extern "C" fn(usize, usize, i32, i32, f32) -> usize;

/// The setter the SDK displaced.
static mut ORIGINAL: Option<SetControl> = None;
/// The control context that came with the surface.
static mut CONTROL: usize = 0;

/// The control-surface object the app last handed to the setter.
#[unsafe(no_mangle)]
pub static mut g_surface: usize = 0;

/// How far past the load address a vtable can point and still be the app's.
const IMAGE_SPAN: usize = 0x1200_0000;

/// Record the displaced setter, at install time.
///
/// # Safety
///
/// Called once, before the hook can run.
pub unsafe fn record(original: usize) {
    // SAFETY: the caller's contract; the address is the trampoline for a setter.
    unsafe {
        ORIGINAL = (original != 0).then(|| core::mem::transmute::<usize, SetControl>(original));
    }
}

/// The app's setter, as a hook: learn the live surface, then pass the call on.
///
/// # Safety
///
/// The app calls this with its own objects.
pub unsafe extern "C" fn hook(
    control: usize,
    surface: usize,
    id: i32,
    op: i32,
    value: f32,
) -> usize {
    let base = image::base();
    let looks_live = surface != 0
        && image::read_pointer(surface)
            .is_some_and(|vtable| (base..=base + IMAGE_SPAN).contains(&vtable));
    // SAFETY: only the UI thread calls the setter.
    unsafe {
        if looks_live {
            g_surface = surface;
            CONTROL = control;
        }
        // The setter tail-calls the app's own, so its result is its caller's.
        match ORIGINAL {
            Some(original) => original(control, surface, id, op, value),
            None => 0,
        }
    }
}

/// The surface the page last used, or zero.
#[must_use]
pub fn surface() -> usize {
    // SAFETY: only the UI thread writes it.
    unsafe { core::ptr::addr_of!(g_surface).read() }
}

/// The control context the page last used, or zero.
#[must_use]
pub fn control() -> usize {
    // SAFETY: only the UI thread writes it.
    unsafe { core::ptr::addr_of!(CONTROL).read() }
}

/// The displaced setter, if the site was installed.
#[must_use]
pub fn original() -> Option<SetControl> {
    // SAFETY: written once at install.
    unsafe { *core::ptr::addr_of!(ORIGINAL) }
}

/// Apply one control op to an explicit surface and context.
///
/// Returns zero when the setter is not installed.
#[must_use]
pub fn apply(control: usize, surface: usize, pad: i32, op: i32, value: f32) -> usize {
    match original() {
        // SAFETY: these are the app's own setter and objects.
        Some(original) => unsafe { original(control, surface, pad, op, value) },
        None => 0,
    }
}

/// Apply one control op to `pad` on the surface the page last used.
///
/// Returns zero when the setter is not installed or the surface is unknown.
#[must_use]
pub fn set(pad: i32, op: i32, value: f32) -> usize {
    let (surface, control, original) = (surface(), control(), original());
    match original {
        // SAFETY: these are the app's own setter and objects.
        Some(original) if surface != 0 => unsafe { original(control, surface, pad, op, value) },
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{control, original, set, surface};

    #[test]
    fn an_uninstalled_setter_is_harmless() {
        assert!(original().is_none());
        assert_eq!(set(0, 0x462, 0.0), 0);
        let _ = (surface(), control());
    }
}
