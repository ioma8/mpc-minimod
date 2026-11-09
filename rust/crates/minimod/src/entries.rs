//! The remaining hooks, each one a call with the app's own function.
//!
//! The app calls these; the mod either passes them straight through or adds the
//! one thing it owns - the octave shift, the soft key's label - and the glue
//! records the displaced function once at install time.

/// The app's sixteen-levels constructor.
type SixteenCtor = Option<unsafe extern "C" fn(usize, usize, usize, usize) -> usize>;
/// The app's sixteen-levels note handler.
type SixteenProc = Option<unsafe extern "C" fn(usize, i32, usize)>;
/// The app's pitch handler.
type PitchOff = Option<unsafe extern "C" fn(usize, u64, u64, i32)>;
/// The app's repaint.
type Repaint = Option<unsafe extern "C" fn(usize, u64, u64)>;
/// The app's label assignment.
type StrAssign = Option<unsafe extern "C" fn(usize, *const u8)>;
/// The app's slice writer, remembered so it can be passed on.
static mut ORIGINAL_WRITER: Option<crate::substitute::Writer> = None;

static mut ORIGINAL_CTOR: SixteenCtor = None;
static mut ORIGINAL_PROC: SixteenProc = None;
static mut ORIGINAL_PITCH: PitchOff = None;
static mut ORIGINAL_REPAINT: Repaint = None;
static mut ORIGINAL_STR_ASSIGN: StrAssign = None;

/// Where the page stores the soft key's label.
static mut FILTER_DEST: usize = 0;

/// Record the displaced hooks, for the glue.
///
/// # Safety
///
/// The constructor calls this once, before any of them can run.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_entries_originals(
    ctor: SixteenCtor,
    proc: SixteenProc,
    pitch: PitchOff,
    repaint: Repaint,
    assign: StrAssign,
    writer: Option<crate::substitute::Writer>,
) {
    // SAFETY: the caller's contract.
    unsafe {
        ORIGINAL_CTOR = ctor;
        ORIGINAL_PROC = proc;
        ORIGINAL_PITCH = pitch;
        ORIGINAL_REPAINT = repaint;
        ORIGINAL_STR_ASSIGN = assign;
        ORIGINAL_WRITER = writer;
    }
}

/// The screen the 16 Levels view last created, for the glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_sixteen_screen() -> usize {
    // SAFETY: only the UI thread writes it.
    unsafe { SIXTEEN }
}

/// The 16 Levels view that was last created.
static mut SIXTEEN: usize = 0;

/// The sixteen-levels constructor: remember the view it made.
///
/// # Safety
///
/// The app calls this with its own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_sixteen_ctor(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // SAFETY: the original is the app's own constructor.
    let Some(original) = (unsafe { *core::ptr::addr_of!(ORIGINAL_CTOR) }) else {
        return 0;
    };
    // SAFETY: the caller passed the app's arguments.
    let view = unsafe { original(a, b, c, d) };
    // SAFETY: only the UI thread writes it.
    unsafe { SIXTEEN = if view != 0 { view } else { a } };
    view
}

/// The sixteen-levels note handler: pass it through.
///
/// # Safety
///
/// The app calls this with its own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_sixteen_proc(self_address: usize, index: i32, out: usize) {
    // SAFETY: the original is the app's own handler.
    if let Some(original) = unsafe { *core::ptr::addr_of!(ORIGINAL_PROC) } {
        // SAFETY: the caller passed the app's arguments.
        unsafe { original(self_address, index, out) };
    }
}

/// The pitch handler: add the octave shift the mod applied.
///
/// # Safety
///
/// The app calls this with its own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_pitch_off(a0: usize, a1: u64, a2: u64, a3: i32) {
    // SAFETY: the shape of the app's own call.
    unsafe {
        crate::levels::minimod_pitch_off(
            *core::ptr::addr_of!(ORIGINAL_PITCH),
            a0,
            a1,
            a2,
            a3,
            crate::ui::octave(),
        );
    }
}

/// The app's repaint, forwarded.
///
/// # Safety
///
/// The app calls this with its own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_repaint(component: usize, a: u64, b: u64) {
    // SAFETY: the original is the app's own repaint.
    if let Some(original) = unsafe { *core::ptr::addr_of!(ORIGINAL_REPAINT) } {
        // SAFETY: the caller passed the app's arguments.
        unsafe { original(component, a, b) };
    }
}

/// The label assignment: remember where the soft key's label lives, and rename it
/// while SHIFT is held.
///
/// # Safety
///
/// `text` must be a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_str_assign(dest: usize, text: *const u8) {
    let mut label = text;
    if !label.is_null() {
        // SAFETY: the caller promises a NUL-terminated string.
        let current = unsafe { core::ffi::CStr::from_ptr(label.cast()) };
        if current.to_bytes() == b"Filter" {
            // SAFETY: only the UI thread writes it.
            unsafe { FILTER_DEST = dest };
            if crate::ui::shift() != 0 {
                label = c"AutoTrim".as_ptr().cast();
            }
        }
    }
    // SAFETY: the original is the app's own assignment.
    if let Some(original) = unsafe { *core::ptr::addr_of!(ORIGINAL_STR_ASSIGN) } {
        // SAFETY: the caller passed the app's arguments.
        unsafe { original(dest, label) };
    }
}

/// The slice writer: hand the block to the substitute.
///
/// # Safety
///
/// The app calls this with its own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_hpss_writer(closure: usize, block: *mut u8) {
    // SAFETY: the original is the app's own writer.
    let original = unsafe { *core::ptr::addr_of!(ORIGINAL_WRITER) };
    // SAFETY: the caller passed the app's arguments.
    unsafe { crate::substitute::minimod_substitute(closure, block, original) };
}

/// Where the soft key's label is stored, for the glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_filter_dest() -> usize {
    // SAFETY: only the UI thread writes it.
    unsafe { FILTER_DEST }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_label_the_mod_renames_is_the_stock_one() {
        assert_eq!(c"Filter".to_bytes(), b"Filter");
        assert_eq!(c"AutoTrim".to_bytes(), b"AutoTrim");
    }
}
