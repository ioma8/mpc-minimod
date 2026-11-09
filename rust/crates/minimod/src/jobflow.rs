//! Render jobs: start one, wait for the worker, then check and write the result.
//!
//! The SDK owns the worker, its completion state and the rendered buffers; the
//! feature owns what to read, what each output means and what to tell the user.
//! `TASK` is the feature's only handle — it is not a result slot or a flag.

use core::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::actions::{ACTION_BASS, ACTION_LOFI, ACTION_REVERB, ACTION_SPLIT, ACTION_VINYL};
use crate::backend::bass::Kind;
use crate::paint::{self, show_toast};
use crate::{pads, selection, status};
use mpc_sample_sdk::platform::extract;
use mpc_sample_sdk::platform::selection::AudioReadError;
use mpc_sample_sdk::render::RenderJob;
use mpc_sample_sdk::task::{Task, TaskError};

/// The one render the UI has started and not finished.
static TASK: Mutex<Option<Task<RenderJob>>> = Mutex::new(None);
/// The synth character of the running render, used for its pad name.
static JOB_KIND: AtomicI32 = AtomicI32::new(0);
/// One failure: what to put in the status line and what to toast.
#[derive(Clone, Copy)]
struct Report {
    /// The complete status line.
    status: &'static str,
    /// Whether `status` is the whole line, rather than `"<action>: <status>"`.
    raw: bool,
    toast: &'static str,
}

/// Lock the feature's task slot, recovering from a poisoned mutex.
fn task_slot() -> MutexGuard<'static, Option<Task<RenderJob>>> {
    TASK.lock().unwrap_or_else(PoisonError::into_inner)
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

/// Whether the job produced usable buffers.
fn broken(job: &RenderJob, effect: bool) -> bool {
    if effect {
        job.outputs
            .first()
            .is_none_or(|output| output.audio.is_empty())
    } else {
        job.outputs.len() < 2 || job.outputs[0].audio.is_empty() || job.outputs[1].audio.is_empty()
    }
}

/// Whether the program still has room for the pads the action needs.
fn room_for_pads(job: &RenderJob, effect: bool) -> Result<(), Report> {
    let required = if effect { 1 } else { 2 };
    if pads::has_room(selection::program(job.source.state), required) {
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

/// The name an action shows in the status line.
#[must_use]
pub fn action_name(action: i32) -> &'static str {
    match action {
        ACTION_REVERB => "Reverb",
        ACTION_VINYL => "Vinyl",
        ACTION_LOFI => "Lo-Fi",
        ACTION_BASS => "Synth",
        _ => "HPSS",
    }
}

/// The name the Extract command gives each action's new pad.
fn names() -> [&'static core::ffi::CStr; 6] {
    [
        c"HPSS Harmonic",
        c"HPSS Percussive",
        c"Reverb",
        c"Vinyl",
        c"Lo-Fi",
        c"Synth C2",
    ]
}

/// The names of the synth characters' rendered C2 notes.
fn synth_names() -> [&'static core::ffi::CStr; 8] {
    [
        c"Bass C2",
        c"Piano C2",
        c"Pads C2",
        c"Lead C2",
        c"Bells C2",
        c"Strings C2",
        c"Voice C2",
        c"Organ C2",
    ]
}

/// Write one rendered buffer through the app's Extract command.
fn extract_one(
    job: &RenderJob,
    output: &mpc_sample_sdk::render::RenderOutput,
    name: &'static core::ffi::CStr,
    end: u32,
    source_frames: u32,
) -> Option<extract::Extracted> {
    extract::run(&extract::Request {
        screen: job.source.screen,
        state: job.source.state,
        name,
        buffer: output.audio.get(..),
        source_frames,
        channels: output.channels,
        start: 0,
        end,
    })
    .ok()
}

/// The op that moves a pad's trim end.
const SET_END: i32 = 0x466;

/// Leave one percent of the rendered tail outside the new pad's trim.
///
/// Extract gives the new pad a range based on the source, which cuts most of a
/// reverb tail. The pad object is already assigned, so move only its end trim
/// through the app's own control surface.
fn trim_reverb_tail(pad: i32, frames: usize) {
    let Ok(frames) = u32::try_from(frames) else {
        return;
    };
    let one_percent = (frames / 100).max(1);
    let end = frames.saturating_sub(one_percent);
    let surface = crate::control::surface();
    if surface == 0 {
        return;
    }
    let _ = crate::control::apply(crate::control::control(), surface, pad, SET_END, end as f32);
}

/// Write an effect or synth take to a pad.
fn extract_take(job: &RenderJob) -> Result<(), Report> {
    let Some(output) = job.outputs.first() else {
        return Err(Report {
            raw: false,
            status: "output failed",
            toast: "Effect: write failed",
        });
    };
    let name = if job.kind == ACTION_BASS {
        let kind = Kind::from_index(JOB_KIND.load(Ordering::Acquire));
        synth_names()[kind.index()]
    } else {
        names()[usize::try_from(job.kind + 1).unwrap_or(5).min(5)]
    };
    // The app's Extract takes the source length for an effect; a synth take may
    // be shorter than its source and then uses its own frame count.
    let rendered = u32::try_from(output.frames).unwrap_or(job.source.frames);
    let end = if job.kind == ACTION_BASS {
        rendered.min(job.source.frames)
    } else {
        job.source.frames
    };
    let source_frames = end;
    let Some(extracted) = extract_one(job, output, name, end, source_frames) else {
        return Err(Report {
            raw: false,
            status: "output failed",
            toast: "Effect: write failed",
        });
    };
    if job.kind == ACTION_REVERB {
        trim_reverb_tail(extracted.pad, output.frames);
    }
    Ok(())
}

/// Write the harmonic and percussive parts to two pads.
fn extract_split(job: &RenderJob) -> Result<(), Report> {
    let (Some(harmonic), Some(percussive)) = (job.outputs.first(), job.outputs.get(1)) else {
        return Err(Report {
            raw: true,
            status: "HPSS: first output failed",
            toast: "HPSS: write failed",
        });
    };
    let frames = u32::try_from(harmonic.frames).unwrap_or(job.source.frames);
    if extract_one(job, harmonic, names()[0], frames, frames).is_none() {
        return Err(Report {
            raw: true,
            status: "HPSS: first output failed",
            toast: "HPSS: write failed",
        });
    }
    if extract_one(job, percussive, names()[1], frames, frames).is_none() {
        return Err(Report {
            raw: true,
            status: "HPSS: harmonic only (second failed)",
            toast: "HPSS: 1 of 2",
        });
    }
    Ok(())
}

/// The success line for an action.
const fn success(job: &RenderJob) -> Report {
    if job.kind == ACTION_SPLIT {
        Report {
            raw: true,
            status: "HPSS: harmonic + percussive created",
            toast: "HPSS done",
        }
    } else {
        Report {
            raw: false,
            status: "copy created",
            toast: match job.kind {
                ACTION_REVERB => "Reverb done",
                ACTION_VINYL => "Vinyl done",
                ACTION_LOFI => "Lo-Fi done",
                _ => "Bass C2 done",
            },
        }
    }
}

/// Check, extract and report one finished job.
fn finalize(job: &RenderJob, screen: usize) -> Report {
    let effect = job.kind != ACTION_SPLIT;
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
    } else if screen != job.source.screen || !job.source.is_current() {
        Err(cancelled(effect))
    } else {
        room_for_pads(job, effect)
            .and_then(|()| {
                let wrote = if effect {
                    extract_take(job)
                } else {
                    extract_split(job)
                };
                crate::ui::minimod_restore_chop(crate::ui::state());
                wrote
            })
            .map(|()| success(job))
    };
    match outcome {
        Ok(report) | Err(report) => report,
    }
}

/// The text for an unavailable input.
fn read_error(action: i32, error: AudioReadError) -> Report {
    let effect = action != ACTION_SPLIT;
    match error {
        AudioReadError::NoPath => Report {
            raw: false,
            status: "no exact WAV file",
            toast: if effect {
                "Effect: no WAV"
            } else {
                "HPSS: no WAV"
            },
        },
        AudioReadError::DecodeFailed => Report {
            raw: false,
            status: "cannot decode WAV",
            toast: if effect {
                "Effect: bad WAV"
            } else {
                "HPSS: bad WAV"
            },
        },
        AudioReadError::LengthMismatch => Report {
            raw: false,
            status: "sample length changed",
            toast: if effect {
                "Effect: bad sample"
            } else {
                "HPSS: bad sample"
            },
        },
    }
}

/// Tell the user why a start failed.
fn fail(action: i32, report: Report) {
    if report.raw {
        status::set_status(report.status);
    } else {
        status::set_prefixed(action_name(action), report.status);
    }
    show_toast(report.toast);
}

/// Start `action` on the selected sample.
pub fn start(action: i32, screen: usize, menu: &mut crate::menu::State) -> bool {
    if task_slot().is_some() {
        return false;
    }
    let Some(source) = selection::current(screen) else {
        fail(
            action,
            Report {
                raw: false,
                status: "no sample on this pad",
                toast: if action == ACTION_SPLIT {
                    "HPSS: no sample"
                } else {
                    "Effect: no sample"
                },
            },
        );
        return false;
    };
    let input = if action == ACTION_BASS {
        Vec::new()
    } else {
        match source.read_audio() {
            Ok(audio) => audio,
            Err(error) => {
                fail(action, read_error(action, error));
                return false;
            }
        }
    };
    let job = RenderJob::new(source, action, input);
    if action == ACTION_BASS {
        JOB_KIND.store(
            i32::try_from(crate::menu::bass_kind().index()).unwrap_or(0),
            Ordering::Release,
        );
    }
    let knobs = crate::menu::bass_knobs();
    let kind = crate::menu::bass_kind();
    let task = Task::spawn(screen, job, move |job| {
        let outcome = crate::actions::render(job, &knobs, kind);
        // The worker's completion is the only event that will reach the UI when
        // the user is idle; ask for the frame that takes the result.
        crate::ui::minimod_repaint_screen();
        outcome
    });
    *task_slot() = Some(task);
    menu.restore_pending = i32::from(crate::ui::screen_chop_mode());
    menu.busy = 1;
    crate::ui::minimod_repaint_screen();
    status::set_status(&format!("Processing {}...", action_name(action)));
    true
}

/// The page the running job started from, or zero when none runs.
#[must_use]
pub fn job_screen() -> usize {
    task_slot().as_ref().map_or(0, Task::key)
}

/// Whether a job is running or waiting to be finished.
#[must_use]
pub fn busy() -> bool {
    task_slot().is_some()
}

/// Clear the menu and preview state after a terminal job.
fn finish_state() {
    let menu = crate::menu::state_mut();
    menu.busy = 0;
    menu.open = 0;
    menu.data_down = 0;
    crate::backend::bass::preview_set_active(false);
    crate::ui::minimod_repaint_screen();
}

/// Finish the job if the worker is done.  Returns whether a job was consumed.
#[must_use]
pub fn finish(screen: usize) -> bool {
    let Some(mut task) = task_slot().take() else {
        return false;
    };
    match task.take() {
        Err(TaskError::Running) => {
            *task_slot() = Some(task);
            false
        }
        Err(TaskError::AlreadyTaken) => {
            finish_state();
            true
        }
        Err(TaskError::Panicked) => {
            status::set_status("processing failed");
            show_toast("processing failed");
            finish_state();
            true
        }
        Err(TaskError::Render(code)) => {
            status::set_prefixed(action_name(code), "processing failed");
            show_toast(if code == ACTION_SPLIT {
                "HPSS: processing failed"
            } else {
                "Effect: processing failed"
            });
            finish_state();
            true
        }
        Ok(job) => {
            let report = finalize(&job, screen);
            if report.raw {
                status::set_status(report.status);
            } else {
                status::set_prefixed(action_name(job.kind), report.status);
            }
            show_toast(report.toast);
            finish_state();
            true
        }
    }
}

/// The status setter in the shape the menu wants.
fn set_status(text: &str) {
    status::set_status(text);
}

/// Start a transform job bound to no effect.
fn start_split(menu: &mut crate::menu::State) {
    start(ACTION_SPLIT, paint::screen(), menu);
}

/// Start a transform job for one effect.
fn start_effect(action: i32, menu: &mut crate::menu::State) {
    start(action, paint::screen(), menu);
}

/// Start a spectral chop.
fn start_spectral(menu: &mut crate::menu::State) {
    let busy = menu.busy != 0 || busy();
    if crate::spectral::run_start(busy) {
        crate::ui::minimod_repaint_screen();
    }
}

/// Open the bass control page.
fn start_bass_page(menu: &mut crate::menu::State) {
    crate::menu::bass_page(menu);
}

/// Start the bass synth.
fn start_bass(menu: &mut crate::menu::State) {
    start(ACTION_BASS, paint::screen(), menu);
}

/// Open the looper, re-reading the device's rings if it opened.
fn start_looper() -> i32 {
    let opened = crate::backend::looper::looper_activate();
    if opened != 0 {
        mpc_sample_sdk::audio::mark_buffers_stale();
        crate::backend::bass::preview_set_active(false);
        mpc_sample_sdk::draw::start_repaint_pump(crate::backend::audio::pump_component);
    }
    opened
}

/// Run the selected menu row.
pub fn menu_activate() {
    let hooks = crate::menu::Hooks {
        repaint: Some(crate::ui::minimod_repaint_screen),
        start_split: Some(start_split),
        start_effect: Some(start_effect),
        start_spectral: Some(start_spectral),
        start_bass: Some(start_bass),
        start_bass_page: Some(start_bass_page),
        start_looper: Some(start_looper),
        set_status: Some(set_status),
    };
    crate::menu::activate(crate::menu::state_mut(), hooks);
}

#[cfg(test)]
mod tests {
    use super::action_name;

    #[test]
    fn action_names_match_the_menu() {
        assert_eq!(action_name(0), "HPSS");
        assert_eq!(action_name(1), "Reverb");
        assert_eq!(action_name(2), "Vinyl");
        assert_eq!(action_name(3), "Lo-Fi");
        assert_eq!(action_name(4), "Synth");
    }
}
