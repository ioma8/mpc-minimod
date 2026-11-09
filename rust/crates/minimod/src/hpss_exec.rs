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
