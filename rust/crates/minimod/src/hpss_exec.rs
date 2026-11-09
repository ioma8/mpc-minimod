//! The slice command's execute step.
//!
//! The mod borrows the app's own Extract command to turn a selection into a new
//! pad.  When the mod armed a substitute buffer, the command's settings object is
//! patched for exactly as long as the app copies its range into the slice, the
//! app runs, the range is put back, and the writer is waited for.

use crate::firmware;
use crate::substitute;

/// Where the settings object keeps the range start.
const RANGE_START: usize = 0x78;
/// Where it keeps the range end.
const RANGE_END: usize = 0xd0;
/// How long the writer may take, in milliseconds.
const WRITER_TIMEOUT_MS: u32 = 3_000;

/// One armed extraction.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Arm {
    /// Whether this extraction replaces the audio at all.
    pub armed: i32,
    /// The float buffer to substitute, or null to let the app copy natively.
    pub buffer: usize,
    /// How many frames it holds.
    pub frames: usize,
    /// How many frames the source has.
    pub source_frames: usize,
    /// The channel count.
    pub channels: u32,
    /// First source frame.
    pub start: u32,
    /// One past the last source frame.
    pub end: u32,
    /// Whether the name should be replaced too.
    pub rename: i32,
}

/// What the caller must apply afterwards.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    /// Whether the writer consumed the whole buffer.
    pub write_ok: i32,
}

/// Run the command, patching its range for the duration of the call.
///
/// # Safety
///
/// `command` must be the app's command object and `execute` its displaced
/// method.
#[must_use]
pub unsafe fn run(
    command: usize,
    arm: Arm,
    execute: Option<unsafe extern "C" fn(usize)>,
) -> Outcome {
    let mut outcome = Outcome::default();
    if arm.armed == 0 {
        if let Some(execute) = execute {
            // SAFETY: the glue's own callback.
            unsafe { execute(command) };
        }
        return outcome;
    }
    substitute::clear();
    if arm.buffer != 0 {
        substitute::arm(
            arm.buffer,
            arm.frames,
            arm.source_frames,
            usize::try_from(arm.channels).unwrap_or(0),
        );
    }
    // The constructor stores the settings object by pointer; the execute method
    // is what copies its range into the slice command.
    let settings = read_u64(command + 0x60).and_then(|value| usize::try_from(value).ok());
    let patched = match settings {
        Some(settings) if settings != 0 => {
            let old_start = read_u64(settings + RANGE_START);
            let old_end = read_u64(settings + RANGE_END);
            match (old_start, old_end) {
                (Some(old_start), Some(old_end)) => {
                    let wrote = write_u64(settings + RANGE_START, u64::from(arm.start))
                        && write_u64(settings + RANGE_END, u64::from(arm.end));
                    if wrote {
                        Some((settings, old_start, old_end))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(execute) = execute {
        // SAFETY: the glue's own callback, running the app's own command.
        unsafe { execute(command) };
    }
    if let Some((settings, old_start, old_end)) = patched {
        let _ = write_u64(settings + RANGE_START, old_start);
        let _ = write_u64(settings + RANGE_END, old_end);
    }
    if !substitute::active() {
        // Nothing was substituted: the app copied its own range.
        outcome.write_ok = i32::from(arm.buffer == 0);
        return outcome;
    }
    // The writer may run on an executor thread: wait for it so the buffer stays
    // alive and the next extraction starts clean.
    let mut spent = 0;
    let mut consumed = substitute::position();
    let total = substitute::total_frames();
    while consumed < total && spent < WRITER_TIMEOUT_MS {
        std::thread::sleep(std::time::Duration::from_millis(1));
        spent += 1;
        consumed = substitute::position();
    }
    substitute::clear();
    outcome.write_ok = i32::from(consumed == total);
    outcome
}

/// Read eight bytes.
fn read_u64(address: usize) -> Option<u64> {
    let mut buffer = [0_u8; 8];
    firmware::read(address, &mut buffer).then(|| u64::from_le_bytes(buffer))
}

/// Write eight bytes.
fn write_u64(address: usize, value: u64) -> bool {
    firmware::write(address, &value.to_le_bytes())
}

/// Run the command, for the C glue.
///
/// # Safety
///
/// `arm` must be readable and `execute` the app's displaced method.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_exec_run(
    command: usize,
    arm: *const Arm,
    execute: Option<unsafe extern "C" fn(usize)>,
) -> Outcome {
    let Some(arm) = (unsafe { arm.as_ref().copied() }) else {
        return Outcome::default();
    };
    // SAFETY: the caller's contract.
    unsafe { run(command, arm, execute) }
}

/// The six effects the mod can render, as the app's own `String` objects.
static mut NAMES: [[u64; 1]; 6] = [[0; 1]; 6];
/// Whether they were built.
static mut NAMES_READY: bool = false;
/// The commands the mod armed, so the execute hook knows which is running.
static mut ARMED_COMMANDS: [usize; 4] = [0; 4];
/// How many of those slots are used.
static mut ARMED_COUNT: usize = 0;

/// The effects' names, in the menu's order.
const NAMES_TEXT: [&core::ffi::CStr; 6] = [
    c"HPSS Harmonic",
    c"HPSS Percussive",
    c"Reverb",
    c"Vinyl",
    c"Lo-Fi",
    c"Bass C2",
];

/// Build the six name objects through the app's own `String` constructor.
///
/// # Safety
///
/// `base` must be the app's load address.
pub unsafe fn make_names(base: usize) {
    // SAFETY: only the UI thread writes the table.
    if unsafe { NAMES_READY } {
        return;
    }
    // SAFETY: the site is the app's `String(const char *)`.
    let construct: unsafe extern "C" fn(usize, *const u8) = unsafe {
        core::mem::transmute::<usize, unsafe extern "C" fn(usize, *const u8)>(base + 0x00f5_35c0)
    };
    for (index, text) in NAMES_TEXT.iter().enumerate() {
        // SAFETY: each slot is a distinct object and the text is static.
        unsafe {
            let slot = core::ptr::addr_of_mut!(NAMES).cast::<u64>().add(index);
            construct(slot as usize, text.as_ptr().cast());
        }
    }
    // SAFETY: see above.
    unsafe { NAMES_READY = true };
}

/// The constructor the app calls for one effect: rename it and remember the
/// command so its execute can be recognised.
///
/// # Safety
///
/// The app calls this with its own arguments, and `forward` is its own
/// constructor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_hpss_ctor(
    command: usize,
    name: usize,
    destination: i32,
    settings: usize,
    pad_state: usize,
    project: usize,
    extra1: usize,
    extra2: usize,
    extra3: usize,
    arm: *const Arm,
    source: usize,
    forward: Option<
        unsafe extern "C" fn(usize, usize, i32, usize, usize, usize, usize, usize, usize),
    >,
) {
    let mut name = name;
    if arm.is_null() {
        return;
    }
    // SAFETY: the caller's contract.
    let arm = unsafe { *arm };
    if arm.armed != 0 && pad_state == source {
        // SAFETY: the names were built during this same install.
        unsafe { make_names(firmware::base()) };
        let index = usize::try_from(arm.rename)
            .ok()
            .filter(|index| *index < 6)
            .unwrap_or(0);
        // SAFETY: the table holds six objects.
        name = unsafe { core::ptr::addr_of!(NAMES).cast::<u64>().add(index) as usize };
        // SAFETY: only the UI thread writes them.
        unsafe {
            if ARMED_COUNT < 4 {
                let slot = core::ptr::addr_of_mut!(ARMED_COMMANDS).cast::<usize>();
                slot.add(ARMED_COUNT).write(command);
                ARMED_COUNT += 1;
            }
        }
    }
    if let Some(forward) = forward {
        // SAFETY: the app's own call.
        unsafe {
            forward(
                command,
                name,
                destination,
                settings,
                pad_state,
                project,
                extra1,
                extra2,
                extra3,
            );
        }
    }
}

/// Whether a command is one the mod armed.
///
/// # Safety
///
/// Read from the UI thread.
/// Forget the commands armed by the last extraction.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_hpss_forget() {
    // SAFETY: only the UI thread writes the list.
    unsafe { ARMED_COUNT = 0 };
}

/// Whether the command is one the mod armed for this extraction.
///
/// # Safety
///
/// Read from the UI thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_is_armed(command: usize) -> i32 {
    // SAFETY: see above.
    i32::from(unsafe {
        let slot = core::ptr::addr_of!(ARMED_COMMANDS).cast::<usize>();
        (0..ARMED_COUNT).any(|index| slot.add(index).read() == command)
    })
}

/// The extraction the mod is about to run, as the hooks need to see it.
static mut ARM: Arm = Arm {
    armed: 0,
    buffer: 0,
    frames: 0,
    source_frames: 0,
    channels: 0,
    start: 0,
    end: 0,
    rename: 0,
};
/// The app's own slice constructor, recorded at install.
static mut ORIGINAL_CTOR: Option<
    unsafe extern "C" fn(usize, usize, i32, usize, usize, usize, usize, usize, usize),
> = None;
/// The app's own slice method, recorded at install.
static mut ORIGINAL_EXEC: Option<unsafe extern "C" fn(usize)> = None;

/// Record the app's slice constructor alone.
///
/// # Safety
///
/// Called once at install.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_ctor_original(constructor: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        ORIGINAL_CTOR = if constructor == 0 {
            None
        } else {
            Some(core::mem::transmute_copy::<usize, _>(&constructor))
        };
    }
}

/// Record the app's slice method alone.
///
/// # Safety
///
/// Called once at install.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_exec_original(execute: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        ORIGINAL_EXEC = if execute == 0 {
            None
        } else {
            Some(core::mem::transmute_copy::<usize, _>(&execute))
        };
    }
}

/// Record the app's slice command, for the hooks.
///
/// # Safety
///
/// Called once at install, before any command runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_originals(constructor: usize, execute: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        ORIGINAL_CTOR = if constructor == 0 {
            None
        } else {
            Some(core::mem::transmute_copy::<usize, _>(&constructor))
        };
        ORIGINAL_EXEC = if execute == 0 {
            None
        } else {
            Some(core::mem::transmute_copy::<usize, _>(&execute))
        };
    }
}

/// The pad state the extraction belongs to.
static mut ARM_SOURCE: usize = 0;
/// Whether the writer consumed the whole buffer.
static mut WRITE_OK: i32 = 0;

/// Arm an extraction: the hooks read this until it is cleared.
///
/// # Safety
///
/// Called on the UI thread while the app runs one Extract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_arm(
    source: usize,
    buffer: usize,
    frames: usize,
    source_frames: usize,
    channels: u32,
    start: u32,
    end: u32,
    rename: i32,
) {
    // SAFETY: the statics are this module's.
    unsafe {
        ARM = Arm {
            armed: 1,
            buffer,
            frames,
            source_frames,
            channels,
            start,
            end,
            rename,
        };
        ARM_SOURCE = source;
    }
}

/// Disarm after the app has run the command.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_hpss_disarm() {
    // SAFETY: the static is this module's.
    unsafe { ARM.armed = 0 };
}

/// Whether the writer consumed the whole buffer, for the caller.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_hpss_write_ok() -> i32 {
    // SAFETY: the static is this module's.
    unsafe { WRITE_OK }
}

/// The slice command's constructor: rename it and remember the command.
///
/// # Safety
///
/// The app calls this with its own arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_ctor_entry(
    one: usize,
    two: usize,
    three: usize,
    four: usize,
    five: usize,
    six: usize,
    seven: usize,
    eight: usize,
    nine: usize,
) {
    let destination = i32::try_from(three).unwrap_or(0);
    // SAFETY: the statics are this module's.
    let (arm, source) = unsafe { (ARM, ARM_SOURCE) };
    // SAFETY: the static is this module's.
    let forward = unsafe { *core::ptr::addr_of!(ORIGINAL_CTOR) };
    // SAFETY: the caller's contract.
    unsafe {
        minimod_hook_hpss_ctor(
            one,
            two,
            destination,
            four,
            five,
            six,
            seven,
            eight,
            nine,
            core::ptr::from_ref(&arm),
            source,
            forward,
        );
    }
}

/// The slice command's execute: run it with the armed buffer, if any.
///
/// # Safety
///
/// The app calls this with its own command.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hpss_exec_entry(command: usize) {
    // SAFETY: the statics are this module's.
    let mut arm = unsafe { ARM };
    arm.armed = i32::from(arm.armed != 0 && unsafe { minimod_hpss_is_armed(command) } != 0);
    // SAFETY: the static is this module's.
    let execute = unsafe { *core::ptr::addr_of!(ORIGINAL_EXEC) };
    // SAFETY: the command and the app's method are the caller's.
    let outcome = unsafe { minimod_hpss_exec_run(command, core::ptr::from_ref(&arm), execute) };
    if arm.armed != 0 {
        // SAFETY: the static is this module's.
        unsafe { WRITE_OK = outcome.write_ok };
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, RANGE_END, RANGE_START};
    use core::mem::size_of;

    #[test]
    fn the_range_slots_are_the_ones_the_app_uses() {
        assert_eq!(RANGE_START, 0x78);
        assert_eq!(RANGE_END, 0xd0);
        assert_eq!(size_of::<Outcome>(), 4);
    }
}
