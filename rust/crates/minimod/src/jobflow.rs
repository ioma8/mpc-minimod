//! Finishing a job: the checks, the extracts and the words the user sees.
//!
//! The worker has already produced its buffers; this decides whether the result
//! can still be used, writes it to pads through the app's own extract, and
//! reports.  The extract itself stays a callback because it drives a firmware
//! handler and waits on the app's writer - the one part that is still glue.

use crate::job::{ACTION_BASS, ACTION_SPLIT, Job};
use crate::{firmware, pads, page, selection, status, wav_ffi};

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
