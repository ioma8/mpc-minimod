//! Render jobs: start one, wait for the worker, then check and write the result.
//!
//! The SDK owns the worker, its completion state and the rendered buffers; the
//! feature owns what to read, what each output means and what to tell the user.
//! `TASK` is the feature's only handle — it is not a result slot or a flag.

use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::ffi::CString;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::actions::{Action, Job};
use crate::backend::bass::Kind;
use crate::paint::{self, show_toast};
use crate::{pads, selection, status};
use mpc_sample_sdk::platform::extract;
use mpc_sample_sdk::platform::selection::AudioReadError;
use mpc_sample_sdk::task::{Task, TaskError};

/// The one render the UI has started and not finished.
static TASK: Mutex<Option<Task<Job>>> = Mutex::new(None);
/// The synth character of the running render, used for its pad name.
static JOB_KIND: AtomicI32 = AtomicI32::new(0);
/// The semitone offset of the running synth render.
static JOB_NOTE: AtomicI32 = AtomicI32::new(0);
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
fn task_slot() -> MutexGuard<'static, Option<Task<Job>>> {
    TASK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Cancel text for an action.
fn cancelled(action: Action) -> Report {
    if action == Action::Bass {
        Report {
            raw: false,
            status: "cancelled",
            toast: "Synth cancelled",
        }
    } else if action.is_effect() {
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
fn broken(job: &Job, effect: bool) -> bool {
    if effect {
        job.outputs.len() != 1 || job.outputs[0].audio.is_empty()
    } else {
        job.outputs.len() != 2 || job.outputs[0].audio.is_empty() || job.outputs[1].audio.is_empty()
    }
}

/// Whether the program still has room for the pads the action needs.
fn room_for_pads(job: &Job, effect: bool) -> Result<(), Report> {
    let required = if effect { 1 } else { 2 };
    // SAFETY: the program came from the live selection that produced this job.
    let Some(source) = job.source.as_ref() else {
        return Err(Report {
            raw: false,
            status: "sample changed",
            toast: "processing cancelled",
        });
    };
    // SAFETY: `source` is the live selection captured for this job.
    if unsafe { pads::has_room(selection::program(source.state()), required) } {
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
    // The SDK retains names for the life of the process; reuse each name.
    static NAMES: Mutex<Vec<&'static core::ffi::CStr>> = Mutex::new(Vec::new());
    if note == 0 {
        return synth_names()[kind.index()];
    }
    let text = format!(
        "{} {}",
        SYNTH_LABELS[kind.index()],
        crate::backend::bass::note_name(note)
    );
    let mut names = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(name) = names.iter().find(|name| name.to_bytes() == text.as_bytes()) {
        return name;
    }
    let name = Box::leak(CString::new(text).unwrap_or_default().into_boxed_c_str());
    names.push(name);
    name
}

/// Write one rendered buffer through the app's Extract command.
fn extract_one(
    job: &Job,
    output: &mpc_sample_sdk::render::RenderOutput,
    name: &'static core::ffi::CStr,
    end: u32,
    source_frames: u32,
    destination: Option<i32>,
) -> Option<extract::Extracted> {
    let source = if let Some(destination) = destination {
        extract::Source::Generated(extract::Generated {
            screen: job.screen,
            name,
            audio: &output.audio,
            channels: output.channels,
            destination,
        })
    } else {
        let selection = job.source.as_ref()?;
        extract::Source::Selection(extract::Request {
            screen: job.screen,
            state: selection.state(),
            name,
            buffer: Some(&output.audio),
            source_frames,
            channels: output.channels,
            start: 0,
            end,
            destination: None,
        })
    };
    // SAFETY: `job.screen` and the selected source state, when required, are
    // live on this UI thread; the rendered buffer outlives the Extract call.
    let result = unsafe { extract::run(source) };
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

fn write_failed(action: Action) -> Report {
    Report {
        raw: false,
        status: "output failed",
        toast: if action == Action::Bass {
            "Synth: write failed"
        } else {
            "Effect: write failed"
        },
    }
}

/// Write an effect or synth take to a pad.
fn extract_take(job: &Job) -> Result<(), Report> {
    let action = Action::from_code(job.kind).unwrap_or(Action::Split);
    let Some(output) = job.outputs.first() else {
        return Err(write_failed(action));
    };
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
        u32::try_from(output.frames).unwrap_or(job.frames)
    } else {
        job.frames
    };
    let source_frames = if action == Action::Bass {
        job.frames
    } else {
        end
    };
    let destination = job.destination;
    let Some(extracted) = extract_one(job, output, name, end, source_frames, destination) else {
        return Err(write_failed(action));
    };
    if action == Action::Bass {
        let destination = extracted.pad;
        // SAFETY: the program is still the live page's program on the UI thread.
        if !job.program.is_some_and(|program| unsafe {
            pads::set_program_layer_active(program, destination, true)
        }) {
            return Err(Report {
                raw: false,
                status: "saved; pad not enabled",
                toast: "Synth saved; pad off",
            });
        }
    }
    if action == Action::Reverb {
        trim_reverb_tail(extracted.pad, output.frames);
    }
    Ok(())
}

/// Write the harmonic and percussive parts to two pads.
fn extract_split(job: &Job) -> Result<(), Report> {
    let (Some(harmonic), Some(percussive)) = (job.outputs.first(), job.outputs.get(1)) else {
        return Err(Report {
            raw: true,
            status: "HPSS: first output failed",
            toast: "HPSS: write failed",
        });
    };
    let frames = u32::try_from(harmonic.frames).unwrap_or(job.frames);
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
fn success(job: &Job) -> Report {
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
                _ => "Synth saved",
            },
        }
    }
}

/// Check, extract and report one finished job.
fn finalize(job: &Job, screen: usize) -> Report {
    let action = Action::from_code(job.kind).unwrap_or(Action::Split);
    let effect = action.is_effect();
    let outcome = if broken(job, effect) {
        Err(Report {
            raw: false,
            status: "processing failed",
            toast: match action {
                Action::Bass => "Synth: processing failed",
                Action::Split => "HPSS: processing failed",
                _ => "Effect: processing failed",
            },
        })
    } else if screen != job.screen
        // SAFETY: the screen is the one recorded by the paint hook in this UI
        // callback and belongs to the live selection.
        || !unsafe { crate::page::visible(job.screen) }
        || (action != Action::Bass
            && job.source.as_ref().is_none_or(|source| {
                // SAFETY: finalization runs on the live page's UI thread.
                !unsafe { source.is_current() }
            }))
    {
        Err(cancelled(action))
    } else {
        let room = if action == Action::Bass {
            let program = selection::page_state(screen).and_then(selection::program);
            let surface = if job.surface != 0 {
                job.surface
            } else {
                crate::control::surface()
            };
            let surface_empty = job
                .destination
                .and_then(|pad| pads::pad_is_empty(surface, pad));
            let destination_is_free = surface_empty.unwrap_or_else(|| {
                program.is_some_and(|program| {
                    // SAFETY: the program is the live page's program on the UI thread.
                    (unsafe { pads::first_free_pad(program) }) == job.destination
                })
            });
            if program != job.program || !destination_is_free {
                Err(cancelled(action))
            } else {
                Ok(())
            }
        } else {
            room_for_pads(job, effect)
        };
        room.and_then(|()| {
            // SAFETY: the source page was validated above and is on the UI thread.
            let chop_before = unsafe { crate::page::chop_mode(screen) };
            let wrote = if action == Action::Split {
                extract_split(job)
            } else {
                extract_take(job)
            };
            // The switch to chop mode may happen after the native command
            // returns, so the restore is armed from the mode the job started in
            // and confirmed against the live mode on the next safe button.
            crate::ui::queue_chop_restore(screen, chop_before);
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
        AudioReadError::Stale => Report {
            raw: false,
            status: "sample changed; cancelled",
            toast: if effect {
                "Effect cancelled"
            } else {
                "HPSS cancelled"
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
    crate::ui::minimod_repaint_screen();
}

/// The last pad the user pressed is preferred when it is empty; otherwise use
/// the first empty pad available on the live surface or program.
fn free_destination(surface: usize, program: usize, last: i32) -> Option<usize> {
    if surface != 0 {
        let empty = |pad: usize| {
            i32::try_from(pad).is_ok_and(|pad| pads::pad_is_empty(surface, pad) == Some(true))
        };
        if let Some(pad) = (0..128)
            .find(|pad| i32::try_from(*pad).is_ok_and(|pad| pad == last) && empty(*pad))
            .or_else(|| (0..128).find(|pad| empty(*pad)))
        {
            return Some(pad);
        }
    }
    // SAFETY: the program is the live Sample page's program on the UI thread.
    unsafe { pads::first_free_pad(program) }.and_then(|pad| usize::try_from(pad).ok())
}

/// Start `action` on the selected sample.
pub fn start(
    action: Action,
    screen: usize,
    menu: &mut crate::menu::State,
    last_pad: i32,
    event_surface: usize,
) -> bool {
    if task_slot().is_some() {
        return false;
    }
    // SAFETY: `screen` is the live Sample page recorded by the paint hook in
    // this UI callback.
    let current = unsafe { selection::current(screen) };
    let source = if action == Action::Bass {
        let Some(program) = selection::page_state(screen).and_then(selection::program) else {
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
        let surface = if event_surface != 0 {
            event_surface
        } else {
            crate::control::surface()
        };
        let Some(destination) = free_destination(surface, program, last_pad) else {
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
        // The generated take only needs page identity and output geometry.
        // The SDK borrows the native seed for the Extract call itself.
        let mut job = Job::generated(screen, action.code());
        job.surface = surface;
        job.program = Some(program);
        job.destination = i32::try_from(destination).ok();
        job
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
        Job::selected(source, action.code())
    };
    let mut job = source;
    if let Some(source) = job.source.as_ref() {
        // SAFETY: the source was captured on this live page and this callback
        // runs on the owning UI thread, preventing replacement during the copy.
        match unsafe { source.read_current() } {
            Ok(audio) => job.input = audio,
            Err(error) => {
                fail(action, read_error(action, error));
                return false;
            }
        }
    }
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
    RUNNING.store(true, Ordering::Release);
    let knobs = crate::menu::bass_knobs();
    let kind = crate::menu::bass_kind();
    let task = Task::spawn(screen, job, move |job| {
        let outcome = crate::actions::render(job, &knobs, kind, synth_note);
        // The worker's completion is the only event that will reach the UI when
        // the user is idle; the repaint pump turns this into a frame on the UI
        // thread, so no app call happens from here.
        crate::ui::request_repaint();
        outcome
    });
    *task_slot() = Some(task);
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

/// Lock-free mirror of [`busy`] for the repaint pump.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Whether a job is running, readable from any thread.
#[must_use]
pub fn running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// Clear the menu and preview state after a terminal job.
fn finish_state() {
    RUNNING.store(false, Ordering::Release);
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
            crate::ui::request_repaint();
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
            if let Some(action) = Action::from_code(code) {
                status::set_prefixed(action_name(action), "processing failed");
                show_toast(match action {
                    Action::Bass => "Synth: processing failed",
                    Action::Split => "HPSS: processing failed",
                    _ => "Effect: processing failed",
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
fn start_split(menu: &mut crate::menu::State, last_pad: i32) {
    start(Action::Split, paint::screen(), menu, last_pad, 0);
}

/// Start a transform job for one effect.
fn start_effect(action: Action, menu: &mut crate::menu::State, last_pad: i32) {
    start(action, paint::screen(), menu, last_pad, 0);
}

/// Start a spectral chop.
fn start_spectral(menu: &mut crate::menu::State, event: &[u8; 16], last_pad: i32) {
    let busy = menu.busy != 0 || busy();
    if crate::spectral::run_start(busy, event, last_pad) {
        menu.busy = 1;
        status::set_status("Analysing sample...");
    }
    crate::ui::minimod_repaint_screen();
}

/// Open the bass control page.
fn start_bass_page(menu: &mut crate::menu::State) {
    crate::menu::bass_page(menu);
}

/// Start the bass synth.
fn start_bass(menu: &mut crate::menu::State, last_pad: i32, surface: usize) {
    start(Action::Bass, paint::screen(), menu, last_pad, surface);
}

/// Open the looper, re-reading the device's rings if it opened.
fn start_looper() -> i32 {
    let opened = crate::backend::looper::looper_activate();
    if opened != 0 {
        crate::looper_lights::show();
        mpc_sample_sdk::audio::mark_buffers_stale();
        crate::backend::bass::preview_set_active(false);
        // The pump started in `on_start` keeps the looper view animating; ask
        // for the frame that draws it for the first time.
        crate::ui::request_repaint();
    } else {
        status::set_status("Looper: not enough memory");
    }
    opened
}

/// Run the selected menu row.
pub fn menu_activate(event: &[u8; 16], last_pad: i32, surface: usize) {
    let hooks = crate::menu::Hooks {
        repaint: Some(crate::ui::minimod_repaint_screen),
        start_split: Some(start_split),
        start_effect: Some(start_effect),
        start_spectral: Some(start_spectral),
        start_bass: Some(start_bass),
        start_bass_page: Some(start_bass_page),
        start_looper: Some(start_looper),
    };
    crate::menu::with_state_mut(|state| {
        crate::menu::activate(state, event, hooks, last_pad, surface);
    });
}

#[cfg(test)]
mod tests {
    use super::{action_name, broken, cancelled, success, write_failed};
    use crate::actions::Job;
    use mpc_sample_sdk::render::RenderOutput;

    fn job(outputs: Vec<RenderOutput>) -> Job {
        let mut job = Job::generated(1, 0);
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
    fn synth_reports_name_the_action_without_assuming_a_note() {
        use crate::actions::Action;
        let mut render = job(Vec::new());
        render.kind = Action::Bass.code();
        assert_eq!(success(&render).toast, "Synth saved");
        assert_eq!(write_failed(Action::Bass).toast, "Synth: write failed");
        assert_eq!(cancelled(Action::Bass).toast, "Synth cancelled");
        assert_eq!(write_failed(Action::Reverb).toast, "Effect: write failed");
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
