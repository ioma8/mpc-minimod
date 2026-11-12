//! Render jobs: start one, wait for the worker, then check and write the result.
//!
//! The SDK owns the worker, its completion state and the rendered buffers; the
//! feature owns what to read, what each output means and what to tell the user.
//! `TASK` is the feature's only handle — it is not a result slot or a flag.

use core::sync::atomic::{AtomicI32, Ordering};
use std::ffi::CString;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::actions::Action;
use crate::backend::bass::Kind;
use crate::paint::{self, show_toast};
use crate::{pads, selection, status};
use mpc_sample_sdk::platform::extract;
use mpc_sample_sdk::platform::selection::AudioReadError;
use mpc_sample_sdk::render::RenderJob;
use mpc_sample_sdk::task::{Task, TaskError};

/// The one render the UI has started and not finished.
static TASK: Mutex<Option<Task<RenderJob>>> = Mutex::new(None);
/// The only worker failure that needs detail the `Task` code cannot carry: the
/// input read happens on the worker, so its error is left here for `finish`.
#[derive(Clone, Copy)]
struct Failure {
    /// The action the job was started for.
    action: Action,
    /// Why the input could not be read.
    error: AudioReadError,
}

/// Written by the worker before it reports [`READ_FAILED`].
static FAILURE: Mutex<Option<Failure>> = Mutex::new(None);
/// The `TaskError::Render` code for an input-read failure; action codes are
/// non-negative, so this cannot collide with one.
const READ_FAILED: i32 = -1;
/// The synth character of the running render, used for its pad name.
static JOB_KIND: AtomicI32 = AtomicI32::new(0);
/// The semitone offset of the running synth render.
static JOB_NOTE: AtomicI32 = AtomicI32::new(0);
/// The destination pad of a synth render.
static JOB_PAD: AtomicI32 = AtomicI32::new(-1);
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

/// Lock the input-read failure slot.
fn failure_slot() -> MutexGuard<'static, Option<Failure>> {
    FAILURE.lock().unwrap_or_else(PoisonError::into_inner)
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
        job.outputs.len() != 1 || job.outputs[0].audio.is_empty()
    } else {
        job.outputs.len() != 2 || job.outputs[0].audio.is_empty() || job.outputs[1].audio.is_empty()
    }
}

/// Whether the program still has room for the pads the action needs.
fn room_for_pads(job: &RenderJob, effect: bool) -> Result<(), Report> {
    let required = if effect { 1 } else { 2 };
    // SAFETY: the program came from the live selection that produced this job.
    if unsafe { pads::has_room(selection::program(job.source.state()), required) } {
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
pub fn action_name(action: Action) -> &'static str {
    match action {
        Action::Reverb => "Reverb",
        Action::Vinyl => "Vinyl",
        Action::LoFi => "Lo-Fi",
        Action::Bass => "Synth",
        Action::Split => "HPSS",
    }
}

/// The names the split gives its two pads.
fn split_names() -> [&'static core::ffi::CStr; 2] {
    [c"HPSS Harmonic", c"HPSS Percussive"]
}

/// The name the Extract command gives one effect's pad.
fn effect_name(action: Action) -> Option<&'static core::ffi::CStr> {
    match action {
        Action::Reverb => Some(c"Reverb"),
        Action::Vinyl => Some(c"Vinyl"),
        Action::LoFi => Some(c"Lo-Fi"),
        Action::Split | Action::Bass => None,
    }
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

/// The display names used when a non-C2 note is rendered.
const SYNTH_LABELS: [&str; 8] = [
    "Bass", "Piano", "Pads", "Lead", "Bells", "Strings", "Voice", "Organ",
];

/// The sample name for a character and semitone offset.
fn synth_name(kind: Kind, note: i32) -> &'static core::ffi::CStr {
    if note == 0 {
        return synth_names()[kind.index()];
    }
    let text = format!(
        "{} {}",
        SYNTH_LABELS[kind.index()],
        crate::backend::bass::note_name(note)
    );
    Box::leak(CString::new(text).unwrap_or_default().into_boxed_c_str())
}

/// Write one rendered buffer through the app's Extract command.
fn extract_one(
    job: &RenderJob,
    output: &mpc_sample_sdk::render::RenderOutput,
    name: &'static core::ffi::CStr,
    end: u32,
    source_frames: u32,
    destination: Option<i32>,
) -> Option<extract::Extracted> {
    // SAFETY: the request names the live page/state from the job and the
    // rendered buffer outlives this call; no other code mutates the program.
    let result = unsafe {
        {
            let request = extract::Request {
                screen: job.source.screen(),
                state: job.source.state(),
                name,
                buffer: output.audio.get(..),
                source_frames,
                channels: output.channels,
                start: 0,
                end,
                destination,
            };
            if let Some(destination) = destination {
                extract::generated(&extract::Generated {
                    screen: request.screen,
                    name,
                    audio: &output.audio,
                    channels: output.channels,
                    destination,
                })
            } else {
                extract::run(&request)
            }
        }
    };
    result.ok()
}

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
    // `control::apply` takes an f32 frame index, exact below 2^24 frames
    // (about 6.3 minutes at 44.1 kHz); longer takes round the tail by a frame.
    // SAFETY: the control context and surface were recorded by the control
    // hook in this UI callback; `pad` is live on that surface.
    unsafe { crate::control::set_trim_end(pad, end) };
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
    let action = Action::from_code(job.kind).unwrap_or(Action::Split);
    let name = if action == Action::Bass {
        let kind = Kind::from_index(JOB_KIND.load(Ordering::Acquire));
        synth_name(kind, JOB_NOTE.load(Ordering::Acquire))
    } else {
        let Some(name) = effect_name(action) else {
            return Err(Report {
                raw: false,
                status: "output failed",
                toast: "Effect: write failed",
            });
        };
        name
    };
    // The app's Extract takes the source length for an effect; a synth take may
    // be shorter than its source and then uses its own frame count.
    let end = if action == Action::Bass {
        // The synth generates its own note; never inherit the source length.
        u32::try_from(output.frames).unwrap_or(job.source.frames())
    } else {
        job.source.frames()
    };
    let source_frames = if action == Action::Bass {
        job.source.frames()
    } else {
        end
    };
    let destination = (action == Action::Bass).then(|| JOB_PAD.load(Ordering::Acquire));
    let Some(extracted) = extract_one(job, output, name, end, source_frames, destination) else {
        return Err(Report {
            raw: false,
            status: "output failed",
            toast: "Effect: write failed",
        });
    };
    if action == Action::Bass {
        let destination = extracted.pad;
        // SAFETY: the surface was recorded by the control hook and `destination`
        // is the pad this job just assigned on that surface.
        if !unsafe { pads::set_layer_active(crate::control::surface(), destination, true) } {
            return Err(Report {
                raw: false,
                status: "layer not enabled",
                toast: "Synth: could not enable pad",
            });
        }
    }
    if action == Action::Reverb {
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
    let frames = u32::try_from(harmonic.frames).unwrap_or(job.source.frames());
    if extract_one(job, harmonic, split_names()[0], frames, frames, None).is_none() {
        return Err(Report {
            raw: true,
            status: "HPSS: first output failed",
            toast: "HPSS: write failed",
        });
    }
    if extract_one(job, percussive, split_names()[1], frames, frames, None).is_none() {
        return Err(Report {
            raw: true,
            status: "HPSS: harmonic only (second failed)",
            toast: "HPSS: 1 of 2",
        });
    }
    Ok(())
}

/// The success line for an action.
fn success(job: &RenderJob) -> Report {
    let action = Action::from_code(job.kind).unwrap_or(Action::Split);
    if action == Action::Split {
        Report {
            raw: true,
            status: "HPSS: harmonic + percussive created",
            toast: "HPSS done",
        }
    } else {
        Report {
            raw: false,
            status: "copy created",
            toast: match action {
                Action::Reverb => "Reverb done",
                Action::Vinyl => "Vinyl done",
                Action::LoFi => "Lo-Fi done",
                _ => "Bass C2 done",
            },
        }
    }
}

/// Check, extract and report one finished job.
fn finalize(job: &RenderJob, screen: usize) -> Report {
    let action = Action::from_code(job.kind).unwrap_or(Action::Split);
    let effect = action.is_effect();
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
    } else if screen != job.source.screen()
        // SAFETY: the screen is the one recorded by the paint hook in this UI
        // callback and belongs to the live selection.
        || !unsafe { crate::page::visible(job.source.screen()) }
        || (action != Action::Bass && !job.source.is_current())
    {
        Err(cancelled(effect))
    } else {
        let room = if action == Action::Bass {
            Ok(())
        } else {
            room_for_pads(job, effect)
        };
        room.and_then(|()| {
            let wrote = if action == Action::Split {
                extract_split(job)
            } else {
                extract_take(job)
            };
            crate::ui::minimod_restore_chop(&crate::ui::state());
            wrote
        })
        .map(|()| success(job))
    };
    match outcome {
        Ok(report) | Err(report) => report,
    }
}

/// The text for an unavailable input.
fn read_error(action: Action, error: AudioReadError) -> Report {
    let effect = action.is_effect();
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
fn fail(action: Action, report: Report) {
    if report.raw {
        status::set_status(report.status);
    } else {
        status::set_prefixed(action_name(action), report.status);
    }
    show_toast(report.toast);
}

/// The pad a synth take may be written to: the last pad the user pressed when
/// it is empty, otherwise the first empty pad.  A synth copy must not overwrite
/// an existing sample, but it should land on the selected pad when that pad is
/// empty.
fn free_destination(surface: usize, last: i32) -> Option<usize> {
    if surface == 0 {
        return None;
    }
    let empty = |pad: usize| {
        i32::try_from(pad).is_ok_and(|pad| pads::sample_for_pad(surface, pad).is_none())
    };
    (0..128)
        .find(|pad| i32::try_from(*pad).is_ok_and(|p| p == last) && empty(*pad))
        .or_else(|| (0..128).find(|pad| empty(*pad)))
}

/// Start `action` on the selected sample.
pub fn start(action: Action, screen: usize, menu: &mut crate::menu::State) -> bool {
    if task_slot().is_some() {
        return false;
    }
    *failure_slot() = None;
    // SAFETY: `screen` is the live Sample page recorded by the paint hook in
    // this UI callback.
    let current = unsafe { selection::current(screen) };
    let source = if action == Action::Bass {
        // Choose the destination before seeding: the seed is written into the
        // selected pad's layer, which would otherwise look occupied.
        let Some(destination) =
            free_destination(crate::control::surface(), crate::input::last_pad())
        else {
            fail(
                action,
                Report {
                    raw: false,
                    status: "no free pad",
                    toast: "Synth: no free pad",
                },
            );
            return false;
        };
        JOB_PAD.store(i32::try_from(destination).unwrap_or(0), Ordering::Release);
        // The generated take only needs page identity and output geometry.
        // The SDK borrows the native seed for the Extract call itself.
        let Some(state) = selection::page_state(screen) else {
            fail(
                action,
                Report {
                    raw: false,
                    status: "no sample to seed Extract",
                    toast: "Synth: no source sample",
                },
            );
            return false;
        };
        let frames = 1;
        // SAFETY: `screen` is the live page recorded by the paint hook and
        // `state` is its live state; a synth take has no selected sample.
        unsafe { selection::Selection::from_parts(screen, state, 0, frames, 2, 44_100) }
    } else {
        let Some(source) = current else {
            fail(
                action,
                Report {
                    raw: false,
                    status: "no sample on this pad",
                    toast: if action == Action::Split {
                        "HPSS: no sample"
                    } else {
                        "Effect: no sample"
                    },
                },
            );
            return false;
        };
        source
    };
    let job = RenderJob::new(source, action.code(), Vec::new());
    let synth_note = if action == Action::Bass {
        crate::backend::bass::last_note_semitones()
    } else {
        0
    };
    if action == Action::Bass {
        JOB_KIND.store(
            i32::try_from(crate::menu::bass_kind().index()).unwrap_or(0),
            Ordering::Release,
        );
        JOB_NOTE.store(synth_note, Ordering::Release);
    }
    // Publish the busy state before the worker can finish: a fast result must
    // never be taken while the menu still looks idle.
    menu.busy = 1;
    let knobs = crate::menu::bass_knobs();
    let kind = crate::menu::bass_kind();
    let task = Task::spawn(screen, job, move |job| {
        // The read is the slow part (a decode or a full memory copy): doing it
        // here keeps the module lock, which the app's UI callback holds, out of
        // the audio callback's way.  The source is a snapshot and the menu stays
        // modal until the job finishes, so the sample cannot normally be
        // replaced under the worker; `finalize` revalidates the selection on the
        // UI thread before Extract, so a replacement is cancelled rather than
        // written.
        if job.kind != Action::Bass.code() {
            match job.source.read_audio() {
                Ok(audio) => job.input = audio,
                Err(error) => {
                    *failure_slot() = Some(Failure {
                        action: Action::from_code(job.kind).unwrap_or(Action::Split),
                        error,
                    });
                    crate::ui::request_repaint();
                    return Err(READ_FAILED);
                }
            }
        }
        let outcome = crate::actions::render(job, &knobs, kind, synth_note);
        // The worker's completion is the only event that will reach the UI when
        // the user is idle; the repaint pump turns this into a frame on the UI
        // thread, so no app call happens from here.
        crate::ui::request_repaint();
        outcome
    });
    *task_slot() = Some(task);
    // Record the mode before the job arms chop mode, so the restore puts back
    // what the user had instead of always turning chop mode off.
    crate::ui::set_chop_before(i32::from(crate::ui::screen_chop_mode()));
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
    let was_bass = crate::menu::state().bass_edit != 0;
    crate::menu::with_state_mut(|menu| {
        menu.busy = 0;
        menu.open = 0;
        menu.bass_edit = 0;
    });
    if was_bass {
        crate::synth_lights::hide();
    }
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
            if let Some(failure) = failure_slot().take() {
                fail(failure.action, read_error(failure.action, failure.error));
            } else if let Some(action) = Action::from_code(code) {
                status::set_prefixed(action_name(action), "processing failed");
                show_toast(if action == Action::Split {
                    "HPSS: processing failed"
                } else {
                    "Effect: processing failed"
                });
            } else {
                status::set_status("processing failed");
                show_toast("processing failed");
            }
            finish_state();
            true
        }
        Ok(job) => {
            let report = finalize(&job, screen);
            if report.raw {
                status::set_status(report.status);
            } else {
                let action = Action::from_code(job.kind).unwrap_or(Action::Split);
                status::set_prefixed(action_name(action), report.status);
            }
            show_toast(report.toast);
            finish_state();
            true
        }
    }
}

/// Start a transform job bound to no effect.
fn start_split(menu: &mut crate::menu::State) {
    start(Action::Split, paint::screen(), menu);
}

/// Start a transform job for one effect.
fn start_effect(action: Action, menu: &mut crate::menu::State) {
    start(action, paint::screen(), menu);
}

/// Start a spectral chop.
fn start_spectral(menu: &mut crate::menu::State, event: &[u8; 16]) {
    let busy = menu.busy != 0 || busy();
    // `run_start` requests its own frame.
    let _ = crate::spectral::run_start(busy, event);
}

/// Open the bass control page.
fn start_bass_page(menu: &mut crate::menu::State) {
    crate::menu::bass_page(menu);
}

/// Start the bass synth.
fn start_bass(menu: &mut crate::menu::State) {
    start(Action::Bass, paint::screen(), menu);
}

/// Open the looper, re-reading the device's rings if it opened.
fn start_looper() -> i32 {
    let opened = crate::backend::looper::looper_activate();
    if opened != 0 {
        mpc_sample_sdk::audio::mark_buffers_stale();
        crate::backend::bass::preview_set_active(false);
        // The pump started in `on_start` keeps the looper view animating; ask
        // for the frame that draws it for the first time.
        crate::ui::request_repaint();
    }
    opened
}

/// Run the selected menu row.
pub fn menu_activate(event: &[u8; 16]) {
    let hooks = crate::menu::Hooks {
        repaint: Some(crate::ui::minimod_repaint_screen),
        start_split: Some(start_split),
        start_effect: Some(start_effect),
        start_spectral: Some(start_spectral),
        start_bass: Some(start_bass),
        start_bass_page: Some(start_bass_page),
        start_looper: Some(start_looper),
    };
    crate::menu::with_state_mut(|state| crate::menu::activate(state, event, hooks));
}

#[cfg(test)]
mod tests {
    use super::{action_name, broken};
    use mpc_sample_sdk::platform::selection::Selection;
    use mpc_sample_sdk::render::{RenderJob, RenderOutput};

    fn job(outputs: Vec<RenderOutput>) -> RenderJob {
        // SAFETY: the test only carries geometry; `broken` reads the outputs.
        let source = unsafe { Selection::from_parts(1, 2, 3, 4, 2, 44_100) };
        let mut job = RenderJob::new(source, 0, Vec::new());
        job.outputs = outputs;
        job
    }

    #[test]
    fn an_effect_needs_exactly_one_non_empty_output() {
        let audio = || RenderOutput::new(vec![0.0; 4], 2).expect("stereo");
        assert!(!broken(&job(vec![audio()]), true));
        assert!(broken(&job(Vec::new()), true));
        assert!(broken(&job(vec![audio(), audio()]), true));
        assert!(broken(
            &job(vec![RenderOutput {
                audio: Vec::new(),
                frames: 0,
                channels: 2,
            }]),
            true
        ));
    }

    #[test]
    fn a_split_needs_both_outputs_non_empty() {
        let audio = || RenderOutput::new(vec![0.0; 4], 2).expect("stereo");
        assert!(!broken(&job(vec![audio(), audio()]), false));
        assert!(broken(&job(vec![audio()]), false));
        let mut empty = audio();
        empty.audio.clear();
        empty.frames = 0;
        assert!(broken(&job(vec![audio(), empty]), false));
    }

    #[test]
    fn action_names_match_the_menu() {
        assert_eq!(action_name(crate::actions::Action::Split), "HPSS");
        assert_eq!(action_name(crate::actions::Action::Reverb), "Reverb");
        assert_eq!(action_name(crate::actions::Action::Vinyl), "Vinyl");
        assert_eq!(action_name(crate::actions::Action::LoFi), "Lo-Fi");
        assert_eq!(action_name(crate::actions::Action::Bass), "Synth");
    }
}

#[cfg(test)]
mod note_name_tests {
    use super::synth_name;
    use crate::backend::bass::Kind;

    #[test]
    fn a_non_c2_render_names_its_note() {
        assert_eq!(synth_name(Kind::Bass, 0).to_str().unwrap(), "Bass C2");
        assert_eq!(synth_name(Kind::Bass, 8).to_str().unwrap(), "Bass G#2");
        assert_eq!(synth_name(Kind::Pads, -8).to_str().unwrap(), "Pads E1");
    }
}
