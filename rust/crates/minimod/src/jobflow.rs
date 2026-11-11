//! Render jobs: start one, wait for the worker, then check and write the result.
//!
//! The SDK owns the worker, its completion state and the rendered buffers; the
//! feature owns what to read, what each output means and what to tell the user.
//! `TASK` is the feature's only handle — it is not a result slot or a flag.

use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::actions::{ACTION_BASS, ACTION_LOFI, ACTION_REVERB, ACTION_SPLIT, ACTION_VINYL};
use crate::backend::bass::Kind;
use crate::paint::{self, show_toast};
use crate::{pads, sample, selection, status};
use mpc_sample_sdk::image;
use mpc_sample_sdk::platform::extract;
use mpc_sample_sdk::platform::selection::AudioReadError;
use mpc_sample_sdk::render::RenderJob;
use mpc_sample_sdk::task::{Task, TaskError};

/// The one render the UI has started and not finished.
static TASK: Mutex<Option<Task<RenderJob>>> = Mutex::new(None);
/// The synth character of the running render, used for its pad name.
static JOB_KIND: AtomicI32 = AtomicI32::new(0);
/// The destination pad of a synth render.
static JOB_PAD: AtomicI32 = AtomicI32::new(-1);
/// Borrowed layer sample/holder for an empty source page.
static BORROW_SLOT: AtomicUsize = AtomicUsize::new(0);
static BORROW_SAMPLE: AtomicUsize = AtomicUsize::new(0);
static BORROW_HOLDER: AtomicUsize = AtomicUsize::new(0);
static BORROW_PROVIDER: AtomicUsize = AtomicUsize::new(0);
static EMPTY_BEFORE: Mutex<[bool; 128]> = Mutex::new([false; 128]);

fn clear_borrow() {
    let slot = BORROW_SLOT.load(Ordering::Acquire);
    let provider = BORROW_PROVIDER.load(Ordering::Acquire);
    if slot != 0 && image::read_u64(slot) == Some(provider as u64) {
        restore_provider();
    } else {
        BORROW_SLOT.store(0, Ordering::Release);
        BORROW_SAMPLE.store(0, Ordering::Release);
        BORROW_HOLDER.store(0, Ordering::Release);
        BORROW_PROVIDER.store(0, Ordering::Release);
    }
}

/// Write one pointer-sized field of a live sample/layer object.
fn write_slot(slot: usize, value: u64) -> bool {
    // SAFETY: `slot` is a field of the app's live sample/layer object and the
    // value is a sample or holder pointer the app itself stores there.
    unsafe { image::write(slot, &value.to_le_bytes()) }
}

fn restore_provider() {
    let slot = BORROW_SLOT.swap(0, Ordering::AcqRel);
    let sample = BORROW_SAMPLE.swap(0, Ordering::AcqRel);
    let holder = BORROW_HOLDER.swap(0, Ordering::AcqRel);
    BORROW_PROVIDER.store(0, Ordering::Release);
    if slot != 0 {
        let _ = write_slot(slot, sample as u64);
        let _ = write_slot(slot + 8, holder as u64);
    }
}

/// Record only pads whose empty state can be read before borrowing.
fn capture_empty_pads(surface: usize) {
    let mut empty = EMPTY_BEFORE.lock().unwrap_or_else(PoisonError::into_inner);
    empty.fill(false);
    let (Some(first), Some(end)) = (
        image::read_pointer(surface + 0x2808),
        image::read_pointer(surface + 0x2810),
    ) else {
        return;
    };
    for (index, item) in empty
        .iter_mut()
        .enumerate()
        .take((end.saturating_sub(first) / 8).min(128))
    {
        let Some(state) = image::read_u64(first + index * 8) else {
            continue;
        };
        if state == 0 {
            *item = true;
            continue;
        }
        let Some(layers) = image::read_pointer(state as usize + 0x22e8) else {
            continue;
        };
        *item = image::read_u64(layers + 0x4a8) == Some(0);
    }
}

fn borrow_provider(screen: usize) -> Option<(usize, u32)> {
    let state = selection::page_state(screen)?;
    let layers = image::read_pointer(state.checked_add(0x22e8)?)?;
    let slot = layers.checked_add(0x4a8)?;
    let short = usize::try_from(image::read_u64(slot)?).ok()?;
    let short_holder = usize::try_from(image::read_u64(slot + 8).unwrap_or(0)).ok()?;
    let surface = crate::control::surface();
    if surface == 0 {
        return None;
    }
    let mut provider = None;
    let mut provider_frames = 0_u32;
    for pad in 0..128 {
        let Some(candidate) = pads::sample_for_pad(surface, pad).filter(|sample| *sample != short)
        else {
            continue;
        };
        let Some(metadata) = image::read_pointer(candidate.checked_add(0x130)?) else {
            continue;
        };
        let Some(frames) = image::read_u32(metadata.checked_add(4)?) else {
            continue;
        };
        if frames >= provider_frames {
            provider = Some(candidate);
            provider_frames = frames;
        }
    }
    let provider = provider?;
    let holder = image::read_pointer(provider.checked_add(0x30)?)?;
    capture_empty_pads(surface);
    if !write_slot(slot, provider as u64) || !write_slot(slot + 8, holder as u64) {
        let _ = write_slot(slot, short as u64);
        let _ = write_slot(slot + 8, short_holder as u64);
        return None;
    }
    BORROW_PROVIDER.store(provider, Ordering::Release);
    BORROW_SLOT.store(slot, Ordering::Release);
    BORROW_SAMPLE.store(short, Ordering::Release);
    BORROW_HOLDER.store(short_holder, Ordering::Release);
    Some((state, provider_frames))
}

/// Borrow a real native sample when no pad can provide one.
fn seed_provider(screen: usize) -> Option<(usize, u32)> {
    let state = selection::page_state(screen)?;
    let slot = image::read_pointer(state.checked_add(0x22e8)?)?.checked_add(0x4a8)?;
    let old_sample = image::read_u64(slot)?;
    let old_holder = image::read_u64(slot + 8)?;
    let seed = sample::silent_seed()?;
    let frames = seed.frames();
    capture_empty_pads(crate::control::surface());
    if !write_slot(slot, seed.sample() as u64) {
        return None;
    }
    if !write_slot(slot + 8, seed.holder() as u64) {
        let _ = write_slot(slot, old_sample);
        return None;
    }
    BORROW_PROVIDER.store(seed.sample(), Ordering::Release);
    BORROW_SLOT.store(slot, Ordering::Release);
    BORROW_SAMPLE.store(old_sample as usize, Ordering::Release);
    BORROW_HOLDER.store(old_holder as usize, Ordering::Release);
    Some((state, frames))
}
/// Remove only new provider copies, preserving every previously populated pad.
fn clear_extra_provider_pads(provider: usize, destination: i32) {
    if provider == 0 {
        return;
    }
    let surface = crate::control::surface();
    let empty = EMPTY_BEFORE.lock().unwrap_or_else(PoisonError::into_inner);
    for (index, was_empty) in empty.iter().enumerate() {
        let Ok(pad) = i32::try_from(index) else {
            continue;
        };
        if *was_empty && pad != destination && pads::sample_for_pad(surface, pad) == Some(provider)
        {
            // SAFETY: `pad` is a live pad on the surface recorded by the
            // control hook, on its owning UI thread.
            let _ = unsafe { crate::control::set(pad, 0x40d, 0.0) };
        }
    }
}
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
    destination: Option<i32>,
) -> Option<extract::Extracted> {
    // SAFETY: the request names the live page/state from the job and the
    // rendered buffer outlives this call; no other code mutates the program.
    unsafe {
        extract::run(&extract::Request {
            screen: job.source.screen(),
            state: job.source.state(),
            name,
            buffer: output.audio.get(..),
            source_frames,
            channels: output.channels,
            start: 0,
            end,
            destination,
        })
    }
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
    // SAFETY: the control context and surface were recorded by the control
    // hook in this UI callback; `pad` is live on that surface.
    let _ = unsafe {
        crate::control::apply(crate::control::control(), surface, pad, SET_END, end as f32)
    };
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
    let rendered = u32::try_from(output.frames).unwrap_or(job.source.frames());
    let end = if job.kind == ACTION_BASS {
        // The synth generates its own note; never inherit the source length.
        rendered
    } else {
        job.source.frames()
    };
    let source_frames = if job.kind == ACTION_BASS {
        job.source.frames()
    } else {
        end
    };
    let destination = (job.kind == ACTION_BASS).then(|| JOB_PAD.load(Ordering::Acquire));
    let Some(extracted) = extract_one(job, output, name, end, source_frames, destination) else {
        return Err(Report {
            raw: false,
            status: "output failed",
            toast: "Effect: write failed",
        });
    };
    if job.kind == ACTION_BASS {
        let destination = JOB_PAD.load(Ordering::Acquire);
        let provider = BORROW_PROVIDER.load(Ordering::Acquire);
        clear_borrow();
        clear_extra_provider_pads(provider, destination);
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
    let frames = u32::try_from(harmonic.frames).unwrap_or(job.source.frames());
    if extract_one(job, harmonic, names()[0], frames, frames, None).is_none() {
        return Err(Report {
            raw: true,
            status: "HPSS: first output failed",
            toast: "HPSS: write failed",
        });
    }
    if extract_one(job, percussive, names()[1], frames, frames, None).is_none() {
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
    } else if screen != job.source.screen()
        // SAFETY: the screen is the one recorded by the paint hook in this UI
        // callback and belongs to the live selection.
        || !unsafe { crate::page::visible(job.source.screen()) }
        || (job.kind != ACTION_BASS && !job.source.is_current())
    {
        Err(cancelled(effect))
    } else {
        let room = if job.kind == ACTION_BASS {
            Ok(())
        } else {
            room_for_pads(job, effect)
        };
        room.and_then(|()| {
            let wrote = if job.kind == ACTION_SPLIT {
                extract_split(job)
            } else {
                extract_take(job)
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
    // SAFETY: `screen` is the live Sample page recorded by the paint hook in
    // this UI callback.
    let current = unsafe { selection::current(screen) };
    let source = if action == ACTION_BASS {
        let (state, frames) = if let Some(current) = &current {
            capture_empty_pads(crate::control::surface());
            BORROW_PROVIDER.store(current.sample(), Ordering::Release);
            (current.state(), current.frames())
        } else {
            let Some((state, frames)) = borrow_provider(screen).or_else(|| seed_provider(screen))
            else {
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
            (state, frames)
        };
        let pad = crate::input::last_pad();
        if !(0..128).contains(&pad) {
            restore_provider();
            fail(
                action,
                Report {
                    raw: false,
                    status: "no destination pad",
                    toast: "Synth: no destination pad",
                },
            );
            return false;
        }
        JOB_PAD.store(pad, Ordering::Release);
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
                    toast: if action == ACTION_SPLIT {
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
    let provider = BORROW_PROVIDER.load(Ordering::Acquire);
    clear_borrow();
    clear_extra_provider_pads(provider, JOB_PAD.load(Ordering::Acquire));
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
        paint::start_repaint_pump(crate::backend::audio::pump_component);
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
