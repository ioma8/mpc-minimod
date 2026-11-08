//! Finishing a job: the checks, the extracts and the words the user sees.
//!
//! The worker has already produced its buffers; this decides whether the result
//! can still be used, writes it to pads through the app's own extract, and
//! reports.  The extract itself stays a callback because it drives a firmware
//! handler and waits on the app's writer - the one part that is still glue.

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use crate::job::{ACTION_BASS, ACTION_SPLIT, Job, PATH_BYTES};
use crate::{firmware, pads, page, paths, selection, status, wav, wav_ffi};

/// The job the worker is running, if any.
static JOB: AtomicPtr<Job> = AtomicPtr::new(core::ptr::null_mut());
/// Whether the worker has finished.
static DONE: AtomicBool = AtomicBool::new(false);
/// Whether the UI thread is finishing the job.
static FINALIZING: AtomicBool = AtomicBool::new(false);
/// The screen the page is on.
static SCREEN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// What the glue has to supply when a job starts.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StartHooks {
    /// The worker finished; the UI thread will pick it up.
    pub finished: Option<unsafe extern "C" fn()>,
    /// Redraw.
    pub repaint: Option<unsafe extern "C" fn()>,
    /// Show a toast.
    pub toast: Option<unsafe extern "C" fn(*const u8)>,
}

/// The app's extract: screen, state, buffer, start, end, source frames, buffer
/// frames, channels, name index, and where the assigned pad lands.
pub type Extract =
    unsafe extern "C" fn(usize, usize, *const f32, u32, u32, u32, u32, u32, i32, *mut i32) -> i32;

/// What the glue has to supply.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Write one buffer through the app's extract.
    pub extract: Option<Extract>,
    /// Put the chop mode back after extracting.
    pub restore_chop: Option<unsafe extern "C" fn()>,
    /// Show a toast.
    pub toast: Option<unsafe extern "C" fn(*const u8)>,
}

/// `[state + 0x2310]` is the app's program object.
const STATE_PROGRAM: usize = 0x2310;

/// One failure: what to put in the status line and what to toast.
#[derive(Clone, Copy)]
struct Report {
    /// The complete status line.
    status: &'static str,
    /// Whether `status` is the whole line, rather than `"<action>: <status>"`.
    raw: bool,
    toast: &'static str,
}

/// Cancel text for an action.
const fn cancelled(effect: bool) -> Report {
    if effect {
        Report {
            raw: false,
            status: "cancelled",
            toast: "Effect cancelled",
        }
    } else {
        Report {
            raw: false,
            status: "cancelled",
            toast: "HPSS cancelled",
        }
    }
}

/// Report the outcome through the toast hook.
///
/// # Safety
///
/// `toast` must accept a NUL-terminated string.
unsafe fn say(toast: Option<unsafe extern "C" fn(*const u8)>, text: &str) {
    if let Some(toast) = toast {
        let mut buffer = [0_u8; 64];
        let take = text.len().min(buffer.len() - 1);
        buffer[..take].copy_from_slice(&text.as_bytes()[..take]);
        // SAFETY: the buffer is NUL-terminated above and outlives the call.
        unsafe { toast(buffer.as_ptr()) };
    }
}

/// Whether the job produced usable buffers.
fn broken(job: &Job, effect: bool) -> bool {
    if effect {
        job.rc != 0 || job.rendered.is_null() || u32::try_from(job.rendered_frames).is_err()
    } else {
        job.rc != 0 || job.harmonic.is_null() || job.percussive.is_null()
    }
}

/// Whether the page and the sample the job rendered are still the ones selected.
fn still_selected(base: usize, job: &Job, screen: usize) -> Result<(), Report> {
    if screen != job.screen {
        return Err(cancelled(job.action != ACTION_SPLIT));
    }
    if !page::visible(base, screen) {
        return Err(cancelled(job.action != ACTION_SPLIT));
    }
    let same = selection::current(base, job.screen).is_some_and(|selected| {
        selected.state == job.state
            && selected.sample == job.sample
            && selected.frames == job.frames
            && u32::from(selected.channels) == job.channels
            && selected.rate == job.rate
    });
    if same {
        Ok(())
    } else {
        Err(cancelled(job.action != ACTION_SPLIT))
    }
}

/// Whether the program still has room for the pads the action needs.
fn room_for_pads(base: usize, job: &Job, effect: bool) -> Result<(), Report> {
    let program = firmware::read_pointer(job.state.wrapping_add(STATE_PROGRAM)).unwrap_or(0);
    let required = if effect { 1 } else { 2 };
    if program != 0 && pads::free_estimate(base, program) >= required {
        return Ok(());
    }
    Err(if effect {
        Report {
            raw: false,
            status: "need 1 empty pad",
            toast: "Effect: need 1 pad",
        }
    } else {
        Report {
            raw: false,
            status: "need 2 empty pads",
            toast: "HPSS: need 2 pads",
        }
    })
}

/// Write an effect or synth take to a pad, restoring the source if it moved.
///
/// # Safety
///
/// `hooks` must hold the app's extract and restore.
unsafe fn extract_take(base: usize, job: &Job, hooks: &Hooks) -> Result<(), Report> {
    let read_frames = if job.action == ACTION_BASS && job.rendered_frames < job.frames as usize {
        u32::try_from(job.rendered_frames).unwrap_or(job.frames)
    } else {
        job.frames
    };
    let channels = if job.rendered_channels == 0 {
        job.channels
    } else {
        job.rendered_channels
    };
    let mut pad = -1;
    let extracted = match hooks.extract {
        // SAFETY: the caller guarantees this is the app's extract.
        Some(extract) => unsafe {
            extract(
                job.screen,
                job.state,
                job.rendered,
                0,
                read_frames,
                read_frames,
                u32::try_from(job.rendered_frames).unwrap_or(0),
                channels,
                job.action + 1,
                core::ptr::addr_of_mut!(pad),
            )
        },
        None => 0,
    };
    if let Some(restore) = hooks.restore_chop {
        // SAFETY: the caller guarantees this is the app's restore.
        unsafe { restore() };
    }
    let source_ok = job.source_audio_backup.is_null() || {
        // SAFETY: the path is NUL-terminated by the glue and the backup is the
        // job's own buffer.
        let restored = unsafe {
            wav_ffi::bass_source_restore(
                job.source_path.as_ptr().cast(),
                job.source_data_offset,
                job.source_audio_backup,
                job.source_audio_bytes,
            )
        };
        restored != 0
    };
    let _ = base;
    if !source_ok {
        return Err(Report {
            raw: false,
            status: "source restore failed",
            toast: "Bass: source restore failed",
        });
    }
    if extracted == 0 {
        return Err(Report {
            raw: false,
            status: "output failed",
            toast: "Effect: write failed",
        });
    }
    Ok(())
}

/// Write the harmonic and percussive parts to two pads.
///
/// # Safety
///
/// `hooks` must hold the app's extract and restore.
unsafe fn extract_split(job: &Job, hooks: &Hooks) -> Result<(), Report> {
    let part = |buffer: *const f32, name: i32| -> bool {
        match hooks.extract {
            // SAFETY: the caller guarantees this is the app's extract.
            Some(extract) => unsafe {
                let mut pad = -1;
                extract(
                    job.screen,
                    job.state,
                    buffer,
                    0,
                    job.frames,
                    job.frames,
                    job.frames,
                    job.channels,
                    name,
                    core::ptr::addr_of_mut!(pad),
                ) != 0
            },
            None => false,
        }
    };
    let first = part(job.harmonic, 0);
    if !first {
        if let Some(restore) = hooks.restore_chop {
            // SAFETY: the caller guarantees this is the app's restore.
            unsafe { restore() };
        }
        return Err(Report {
            raw: true,
            status: "HPSS: first output failed",
            toast: "HPSS: write failed",
        });
    }
    let second = part(job.percussive, 1);
    if let Some(restore) = hooks.restore_chop {
        // SAFETY: the caller guarantees this is the app's restore.
        unsafe { restore() };
    }
    if !second {
        return Err(Report {
            raw: true,
            status: "HPSS: harmonic only (second failed)",
            toast: "HPSS: 1 of 2",
        });
    }
    Ok(())
}

/// The success line for an action.
const fn success(job: &Job) -> Report {
    if job.action == ACTION_SPLIT {
        Report {
            raw: true,
            status: "HPSS: harmonic + percussive created",
            toast: "HPSS done",
        }
    } else {
        Report {
            raw: false,
            status: "copy created",
            toast: match job.action {
                1 => "Reverb done",
                2 => "Vinyl done",
                3 => "Lo-Fi done",
                _ => "Bass C2 done",
            },
        }
    }
}

/// Finish a job: check it, extract it, report it, and free it.
///
/// # Safety
///
/// `job` must be the live job the worker finished, `hooks` must hold the app's
/// extract and restore, and `screen` must be the page the job started from.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_job_finalize(
    job: *mut Job,
    hooks: *const Hooks,
    screen: usize,
    base: usize,
) -> i32 {
    if job.is_null() || hooks.is_null() {
        return 1;
    }
    // SAFETY: the caller promises both pointers are live for this call.
    let job = unsafe { &mut *job };
    let hooks = unsafe { *hooks };
    let effect = job.action != ACTION_SPLIT;
    let outcome = if broken(job, effect) {
        Err(Report {
            raw: false,
            status: "processing failed",
            toast: if effect {
                "Effect: processing failed"
            } else {
                "HPSS: processing failed"
            },
        })
    } else {
        still_selected(base, job, screen)
            .and_then(|()| room_for_pads(base, job, effect))
            .and_then(|()| {
                // SAFETY: see the function contract.
                unsafe {
                    if effect {
                        extract_take(base, job, &hooks)
                    } else {
                        extract_split(job, &hooks)
                    }
                }
            })
            .map(|()| success(job))
    };
    let report = match outcome {
        Ok(report) | Err(report) => report,
    };
    if report.raw {
        status::set_status(report.status);
    } else {
        status::set_action_status(job.action, report.status);
    }
    // SAFETY: see the function contract.
    unsafe { say(hooks.toast, report.toast) };
    // SAFETY: the job is live and about to be abandoned.
    unsafe { crate::job::minimod_job_free(core::ptr::from_mut(job)) };
    0
}

/// Fill a fresh job from the selection.
fn fill(
    job: &mut Job,
    selected: &selection::Selection,
    path: &std::path::Path,
    input: *mut f32,
    action: i32,
) {
    job.screen = selected.screen;
    job.state = selected.state;
    job.sample = selected.sample;
    job.frames = selected.frames;
    job.channels = u32::from(selected.channels);
    job.rate = selected.rate;
    job.input = input;
    job.action = action;
    job.rc = -1;
    let bytes = path.to_string_lossy();
    let take = bytes.len().min(PATH_BYTES - 1);
    job.source_path[..take].copy_from_slice(&bytes.as_bytes()[..take]);
}

/// Copy a slice into a `malloc` block, which the job frees with libc `free`.
fn into_malloc(values: &[f32]) -> *mut f32 {
    let bytes = values.len().saturating_mul(size_of::<f32>()).max(1);
    // SAFETY: a plain allocation, checked for null.
    let buffer = unsafe { libc::malloc(bytes) }.cast::<f32>();
    if buffer.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: `buffer` holds `bytes` and the source is a live slice.
    unsafe { core::ptr::copy_nonoverlapping(values.as_ptr(), buffer, values.len()) };
    buffer
}

/// Start `action` on the selected sample.
///
/// Returns the job when it started, so the glue can keep its busy flag; on
/// failure the status line and the toast are already set and null comes back.
///
/// # Safety
///
/// `knobs` must be readable for three values and `hooks` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_job_start(
    action: i32,
    base: usize,
    knobs: *const f32,
    hooks: *const StartHooks,
) -> *mut Job {
    if hooks.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller promises the hooks are live for this call.
    let hooks = unsafe { *hooks };
    if JOB.load(Ordering::Acquire).is_null() {
        // Nothing is running: try to start.
    } else {
        return core::ptr::null_mut();
    }
    let Some(selected) = selection::current(base, current_screen()) else {
        status::set_action_status(action, "no sample on this pad");
        // SAFETY: see the function contract.
        unsafe {
            say(
                hooks.toast,
                if action == 0 {
                    "HPSS: no sample"
                } else {
                    "Effect: no sample"
                },
            );
        }
        return core::ptr::null_mut();
    };
    let Some(path) = paths::for_sample(
        selected.sample,
        selected.frames,
        selected.channels,
        selected.rate,
    ) else {
        status::set_action_status(action, "no exact WAV file");
        // SAFETY: see the function contract.
        unsafe {
            say(
                hooks.toast,
                if action == 0 {
                    "HPSS: no WAV"
                } else {
                    "Effect: no WAV"
                },
            );
        }
        return core::ptr::null_mut();
    };
    let Some(job) = (unsafe { prepare(action, &selected, &path, hooks) }) else {
        return core::ptr::null_mut();
    };
    status::set_status(&format!("Processing {}...", status::action_name(action)));
    JOB.store(job, Ordering::Release);
    DONE.store(false, Ordering::Release);
    let knobs = if knobs.is_null() {
        [0.5_f32, 0.5, 0.5]
    } else {
        // SAFETY: the caller promises three readable values.
        let values = unsafe { core::slice::from_raw_parts(knobs, 3) };
        [values[0], values[1], values[2]]
    };
    let job_address = job as usize;
    std::thread::spawn(move || {
        // SAFETY: the job is owned by the flow until the UI thread frees it, and
        // the address is stable for its whole life.
        unsafe { crate::job::minimod_job_run(job_address as *mut Job, knobs.as_ptr(), base) };
        DONE.store(true, Ordering::Release);
        if let Some(finished) = hooks.finished {
            // SAFETY: the caller's callback.
            unsafe { finished() };
        }
        if let Some(repaint) = hooks.repaint {
            // SAFETY: the caller's callback.
            unsafe { repaint() };
        }
    });
    job
}

/// Whether a job is running or waiting to be finished.
#[must_use]
pub fn busy() -> bool {
    !JOB.load(Ordering::Acquire).is_null()
}

/// Whether a job is running, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_job_busy() -> i32 {
    i32::from(busy())
}

/// Finish the job if the worker is done.
///
/// Returns 1 when a job was consumed, 0 when there was nothing to do.
///
/// # Safety
///
/// `hooks` must be live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_job_finish(
    hooks: *const Hooks,
    screen: usize,
    base: usize,
) -> i32 {
    if hooks.is_null() || FINALIZING.swap(true, Ordering::AcqRel) {
        return 0;
    }
    if !DONE.swap(false, Ordering::AcqRel) {
        FINALIZING.store(false, Ordering::Release);
        return 0;
    }
    let job = JOB.swap(core::ptr::null_mut(), Ordering::AcqRel);
    if job.is_null() {
        FINALIZING.store(false, Ordering::Release);
        return 0;
    }
    // SAFETY: the pointer came from `minimod_job_start` and is ours now.
    unsafe { minimod_job_finalize(job, hooks, screen, base) };
    FINALIZING.store(false, Ordering::Release);
    1
}

/// Allocate the job and give it its input (or, for the bass synth, a backup of
/// the source it will overwrite).
///
/// # Safety
///
/// `hooks` must be live for this call.
unsafe fn prepare(
    action: i32,
    selected: &selection::Selection,
    path: &std::path::Path,
    hooks: StartHooks,
) -> Option<*mut Job> {
    let synth = action == ACTION_BASS;
    let input = if synth {
        core::ptr::null_mut()
    } else {
        let Some(samples) = wav::read_f32(path, selected.frames, selected.channels, selected.rate)
        else {
            status::set_action_status(action, "cannot decode WAV");
            // SAFETY: see the caller's contract.
            unsafe { say_failure(hooks.toast, action, "bad WAV") };
            return None;
        };
        into_malloc(&samples)
    };
    // SAFETY: a plain allocation, checked for null.
    let job = unsafe { libc::calloc(1, size_of::<Job>()) }.cast::<Job>();
    if (input.is_null() && !synth) || job.is_null() {
        // SAFETY: both came from `malloc`; nothing else owns them.
        unsafe {
            libc::free(input.cast());
            libc::free(job.cast());
        }
        status::set_action_status(action, "out of memory");
        return None;
    }
    // SAFETY: `job` is a fresh zeroed allocation we own.
    let fields = unsafe { &mut *job };
    fill(fields, selected, path, input, action);
    if synth {
        // SAFETY: the path is NUL-terminated and the outputs are the job's own
        // fields, which it frees later.
        let backed_up = unsafe {
            wav_ffi::bass_source_backup(
                fields.source_path.as_ptr().cast(),
                core::ptr::addr_of_mut!(fields.source_data_offset),
                core::ptr::addr_of_mut!(fields.source_audio_backup),
                core::ptr::addr_of_mut!(fields.source_audio_bytes),
            )
        };
        if backed_up == 0 {
            // SAFETY: the job owns its buffers.
            unsafe { crate::job::minimod_job_free(job) };
            status::set_action_status(action, "cannot preserve source sample");
            // SAFETY: see the caller's contract.
            unsafe { say(hooks.toast, "Bass: source read failed") };
            return None;
        }
        fields.rendered_channels = 2;
    }
    Some(job)
}

/// The "bad WAV" toast for an action.
///
/// # Safety
///
/// `toast` must accept a NUL-terminated string.
unsafe fn say_failure(toast: Option<unsafe extern "C" fn(*const u8)>, action: i32, what: &str) {
    let text = if action == 0 { "HPSS: " } else { "Effect: " };
    let mut joined = [0_u8; 48];
    let head = text.len();
    joined[..head].copy_from_slice(text.as_bytes());
    let tail = what.len().min(joined.len() - head - 1);
    joined[head..head + tail].copy_from_slice(&what.as_bytes()[..tail]);
    // SAFETY: the buffer is NUL-terminated above and outlives the call.
    unsafe {
        say(
            toast,
            core::str::from_utf8(&joined[..head + tail]).unwrap_or(""),
        );
    }
}

/// Record the current screen; the glue calls this when it changes.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_job_set_screen(screen: usize) {
    SCREEN.store(screen as *mut (), Ordering::Release);
}

/// The screen the page is on.
fn current_screen() -> usize {
    SCREEN.load(Ordering::Acquire) as usize
}
