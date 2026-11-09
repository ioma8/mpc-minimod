//! Running the app's own Extract command with a rendered buffer.
//!
//! The app's Extract reads the selected sample, writes a WAV and assigns a pad.
//! A mod that rendered its own audio arms a substitute buffer: the writer hook
//! swaps each block's pointer for the rendered one, and the command's range is
//! patched for the duration of the call.  The app still owns the pad, the WAV
//! and the command, so this is the supported way to create a pad.
//!
//! The SDK owns the two command hooks (constructor and execute), the writer, and
//! the wait for the executor; a mod only describes the request.

use core::ffi::CStr;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::image;
use crate::platform::pads;
use crate::platform::writer;

/// Where the command's settings object keeps the range start.
const RANGE_START: usize = 0x78;
/// Where it keeps the range end.
const RANGE_END: usize = 0xd0;
/// How long the writer may take, in milliseconds.
const WRITER_TIMEOUT_MS: u32 = 3_000;
/// The app's Extract handler.
const EXTRACT_HANDLER: usize = 0x0165_80d0;
/// `[state + 0x2310]` is the app's program object.
const STATE_PROGRAM: usize = 0x2310;
/// `juce::String(const char *)`.
const STRING_FROM_C: usize = 0x00f5_35c0;
/// How many distinct names the builder keeps.
const MAX_NAMES: usize = 8;
/// How many commands one extraction may arm.
const MAX_ARMED: usize = 4;

/// One extraction.
pub struct Request<'a> {
    /// The Sample page.
    pub screen: usize,
    /// The page state the selection came from.
    pub state: usize,
    /// The name the new pad gets, e.g. `c"HPSS Harmonic"`.
    pub name: &'static CStr,
    /// The rendered audio, interleaved; `None` copies the app's own range.
    pub buffer: Option<&'a [f32]>,
    /// The source sample's length in frames.
    pub source_frames: u32,
    /// The rendered audio's channel count.
    pub channels: u32,
    /// First source frame.
    pub start: u32,
    /// One past the last source frame.
    pub end: u32,
}

/// The pad the app assigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extracted {
    /// The pad the app assigned.
    pub pad: i32,
}

/// Why an extraction did not produce a pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The request's range or channels are unusable.
    Invalid,
    /// The page has no program or the state is stale.
    NoProgram,
    /// The app did not assign a pad, or its writer did not consume the buffer.
    NotApplied,
}

impl core::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Invalid => f.write_str("invalid extract range"),
            Self::NoProgram => f.write_str("no program on this page"),
            Self::NotApplied => f.write_str("the app did not assign a pad"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// One armed extraction, as the hooks see it.
#[derive(Clone, Copy, Debug, Default)]
struct Arm {
    buffer: usize,
    frames: usize,
    source_frames: usize,
    channels: u32,
    start: u32,
    end: u32,
    name: usize,
}

/// The armed extraction.
static mut ARM: Arm = Arm {
    buffer: 0,
    frames: 0,
    source_frames: 0,
    channels: 0,
    start: 0,
    end: 0,
    name: 0,
};
/// Whether `ARM` is valid; release/acquire around the fields.
static ARM_READY: AtomicI32 = AtomicI32::new(0);
/// The pad state the extraction belongs to.
static mut ARM_SOURCE: usize = 0;
/// Whether the writer consumed the whole buffer.
static ARM_WRITE_OK: AtomicI32 = AtomicI32::new(0);
/// The commands the constructor armed for this extraction.
static mut ARMED: [usize; MAX_ARMED] = [0; MAX_ARMED];
/// How many entries of `ARMED` are live.
static mut ARMED_COUNT: usize = 0;
/// The app's own command constructor.
static mut ORIGINAL_CTOR: Option<
    unsafe extern "C" fn(usize, usize, i32, usize, usize, usize, usize, usize, usize),
> = None;
/// The app's own command method.
static mut ORIGINAL_EXEC: Option<unsafe extern "C" fn(usize) -> i32> = None;
/// Persistent `juce::String` objects for the names, built through the app.
static mut NAMES: [[u64; 1]; MAX_NAMES] = [[0; 1]; MAX_NAMES];
/// The C string each name object was built from.
static mut NAME_TEXT: [*const u8; MAX_NAMES] = [core::ptr::null(); MAX_NAMES];
/// How many name objects are built.
static mut NAME_COUNT: usize = 0;

/// Record the displaced command hooks, at install time.
///
/// # Safety
///
/// Called once, before the hooks can run.
pub unsafe fn record(constructor: usize, execute: usize) {
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

/// The name object for `text`, building it once.
fn name_object(text: &'static CStr) -> Option<usize> {
    // SAFETY: only the UI thread calls this.
    unsafe {
        let pointer = text.as_ptr().cast::<u8>();
        let table = core::ptr::addr_of_mut!(NAMES).cast::<u64>();
        let texts = core::ptr::addr_of_mut!(NAME_TEXT).cast::<*const u8>();
        for index in 0..NAME_COUNT {
            if texts.add(index).read() == pointer {
                return Some(table.add(index) as usize);
            }
        }
        if NAME_COUNT == MAX_NAMES {
            return None;
        }
        let slot = table.add(NAME_COUNT);
        let construct: unsafe extern "C" fn(usize, *const u8) =
            core::mem::transmute(image::base() + STRING_FROM_C);
        construct(slot as usize, pointer);
        texts.add(NAME_COUNT).write(pointer);
        NAME_COUNT += 1;
        Some(slot as usize)
    }
}

/// The arm's `armed` flag as an atomic.
fn armed() -> &'static AtomicI32 {
    &ARM_READY
}

/// Read the arm after an acquire load of its flag.
fn snapshot() -> (Arm, usize) {
    if armed().load(Ordering::Acquire) == 0 {
        return (Arm::default(), 0);
    }
    // SAFETY: the release store published every field before the flag.
    unsafe { (*core::ptr::addr_of!(ARM), *core::ptr::addr_of!(ARM_SOURCE)) }
}

/// Whether `command` is one this extraction armed.
fn is_armed(command: usize) -> bool {
    // SAFETY: only the UI thread writes the list, and the constructor ran on it.
    unsafe {
        let slot = core::ptr::addr_of!(ARMED).cast::<usize>();
        (0..ARMED_COUNT).any(|index| slot.add(index).read() == command)
    }
}

/// The slice command's constructor: rename it and remember the command.
///
/// # Safety
///
/// The app calls this with its own arguments.
pub unsafe extern "C" fn ctor_entry(
    command: usize,
    name: usize,
    destination: usize,
    settings: usize,
    pad_state: usize,
    project: usize,
    extra1: usize,
    extra2: usize,
    extra3: usize,
) {
    let (arm, source) = snapshot();
    let mut chosen = name;
    if arm.name != 0 && pad_state == source {
        chosen = arm.name;
        // SAFETY: only the UI thread writes the list; the constructor ran on it.
        unsafe {
            if ARMED_COUNT < MAX_ARMED {
                core::ptr::addr_of_mut!(ARMED)
                    .cast::<usize>()
                    .add(ARMED_COUNT)
                    .write(command);
                ARMED_COUNT += 1;
            }
        }
    }
    // SAFETY: the static is the app's own constructor.
    if let Some(forward) = unsafe { *core::ptr::addr_of!(ORIGINAL_CTOR) } {
        // SAFETY: the app's own call.
        unsafe {
            forward(
                command,
                chosen,
                i32::try_from(destination).unwrap_or(0),
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

/// The slice command's execute: run it with the armed buffer, if any.
///
/// # Safety
///
/// The app calls this with its own command.
#[must_use]
pub unsafe extern "C" fn exec_entry(command: usize) -> i32 {
    let (arm, _) = snapshot();
    // SAFETY: the static is the app's own method.
    let execute = unsafe { *core::ptr::addr_of!(ORIGINAL_EXEC) };
    if arm.name == 0 || !is_armed(command) {
        return match execute {
            // SAFETY: the app's own method.
            Some(execute) => unsafe { execute(command) },
            None => -1,
        };
    }
    // SAFETY: the command and the app's method are the caller's.
    unsafe { run_command(command, arm, execute) }
}

/// Run one armed command, patching its range for the duration of the call.
///
/// # Safety
///
/// `command` must be the app's command and `execute` its displaced method.
unsafe fn run_command(
    command: usize,
    arm: Arm,
    execute: Option<unsafe extern "C" fn(usize) -> i32>,
) -> i32 {
    writer::clear();
    if arm.buffer != 0 {
        writer::arm(
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
                    let start_written = write_u64(settings + RANGE_START, u64::from(arm.start));
                    let end_written = write_u64(settings + RANGE_END, u64::from(arm.end));
                    Some((
                        settings,
                        start_written.then_some(old_start),
                        end_written.then_some(old_end),
                    ))
                }
                _ => None,
            }
        }
        _ => None,
    };
    let status = match execute {
        // SAFETY: the app's own method.
        Some(execute) => unsafe { execute(command) },
        None => -1,
    };
    if let Some((settings, old_start, old_end)) = patched {
        if let Some(old) = old_start {
            let _ = write_u64(settings + RANGE_START, old);
        }
        if let Some(old) = old_end {
            let _ = write_u64(settings + RANGE_END, old);
        }
    }
    if !writer::active() {
        ARM_WRITE_OK.store(i32::from(arm.buffer == 0), Ordering::Release);
        return status;
    }
    let mut spent = 0;
    let mut consumed = writer::position();
    let total = writer::total_frames();
    while consumed < total && spent < WRITER_TIMEOUT_MS {
        std::thread::sleep(core::time::Duration::from_millis(1));
        spent += 1;
        consumed = writer::position();
    }
    writer::clear();
    writer::wait_idle(WRITER_TIMEOUT_MS);
    ARM_WRITE_OK.store(i32::from(consumed == total), Ordering::Release);
    status
}

/// Read eight bytes.
fn read_u64(address: usize) -> Option<u64> {
    let mut buffer = [0_u8; 8];
    image::read(address, &mut buffer).then(|| u64::from_le_bytes(buffer))
}

/// Write eight bytes.
fn write_u64(address: usize, value: u64) -> bool {
    image::write(address, &value.to_le_bytes())
}

/// Run one native Extract.
///
/// With a buffer the writer substitutes it; without one the app copies its own
/// range.  The call blocks until the app has assigned a pad (or three seconds
/// have passed), because the command may run on an executor thread.
///
/// # Errors
///
/// Returns [`ExtractError`] when the request is unusable, the page has no
/// program, or the app did not assign a pad.
pub fn run(request: &Request<'_>) -> Result<Extracted, ExtractError> {
    if request.screen == 0 || request.start >= request.end || request.channels == 0 {
        return Err(ExtractError::Invalid);
    }
    let Some(program) = image::read_u64(
        request
            .state
            .checked_add(STATE_PROGRAM)
            .ok_or(ExtractError::Invalid)?,
    ) else {
        return Err(ExtractError::NoProgram);
    };
    let program = usize::try_from(program).unwrap_or(0);
    if program == 0 {
        return Err(ExtractError::NoProgram);
    }
    let before = pads::free_count(program);
    if before < 0 {
        return Err(ExtractError::NoProgram);
    }
    let Some(name) = name_object(request.name) else {
        return Err(ExtractError::Invalid);
    };
    let frames = request.buffer.map_or(
        usize::try_from(request.end - request.start).unwrap_or(0),
        |buffer| buffer.len() / usize::try_from(request.channels).unwrap_or(1),
    );
    ARM_WRITE_OK.store(0, Ordering::Release);
    // SAFETY: the fields are written before the release flag; the hooks acquire.
    unsafe {
        core::ptr::addr_of_mut!(ARM).write(Arm {
            buffer: request.buffer.map_or(0, |buffer| buffer.as_ptr() as usize),
            frames,
            source_frames: usize::try_from(request.source_frames).unwrap_or(0),
            channels: request.channels,
            start: request.start,
            end: request.end,
            name,
        });
        core::ptr::addr_of_mut!(ARM_SOURCE).write(request.state);
        core::ptr::addr_of_mut!(ARMED_COUNT).write(0);
        armed().store(1, Ordering::Release);
        let handler: unsafe extern "C" fn(usize) =
            core::mem::transmute(image::base() + EXTRACT_HANDLER);
        handler(request.screen);
    }
    let mut after = before;
    for _ in 0..3000 {
        if after != before {
            break;
        }
        std::thread::sleep(core::time::Duration::from_millis(1));
        after = pads::free_count(program);
    }
    armed().store(0, Ordering::Release);
    let wrote = if request.buffer.is_none() {
        true
    } else {
        ARM_WRITE_OK.load(Ordering::Acquire) != 0
    };
    if after != before && wrote {
        Ok(Extracted { pad: before })
    } else {
        Err(ExtractError::NotApplied)
    }
}

#[cfg(test)]
mod tests {
    use super::{ExtractError, RANGE_END, RANGE_START, Request};

    #[test]
    fn the_range_slots_are_the_apps() {
        assert_eq!(RANGE_START, 0x78);
        assert_eq!(RANGE_END, 0xd0);
    }

    #[test]
    fn an_empty_range_is_refused_before_touching_the_app() {
        let request = Request {
            screen: 0x1000,
            state: 0x2000,
            name: c"test",
            buffer: None,
            source_frames: 0,
            channels: 1,
            start: 10,
            end: 10,
        };
        assert_eq!(super::run(&request).unwrap_err(), ExtractError::Invalid);
    }
}
