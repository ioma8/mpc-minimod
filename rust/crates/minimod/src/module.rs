//! The mod entry point: bind the feature policy to the SDK.
//!
//! Every hook, the firmware image and the audio taps belong to the SDK.  This
//! type only decides what the features do with the events.

use mpc_sample_sdk::{
    AudioHooks, ButtonEvent, CcEvent, EncoderEvent, Flow, InputHooks, Mod, NoteEvent, PadEvent,
    PadResponse, PaintEvent, SurfaceEvent,
};

/// The mod owns cross-event input history; feature state stays with its feature.
#[derive(Default)]
pub struct Minimod {
    input: crate::input::State,
}

impl Mod for Minimod {
    fn on_start(&mut self) {
        // The preview mixes from the note hooks, so it exists only when both
        // were installed.
        crate::backend::bass::set_available(mpc_sample_sdk::midi::notes_ready());
        crate::settings::load_into_mod();
        crate::recent::start();
        // One pump for the life of the process: the looper's animation and the
        // single frame a finished worker asks for both go through it, so no
        // worker ever calls the app's UI from its own thread.
        crate::paint::start_repaint_pump(crate::ui::pump_component);
    }

    fn on_encoder(&mut self, event: EncoderEvent) -> Flow {
        crate::ui::cancel_chop_restore();
        let decision = crate::input::encoder(&self.input, event);
        match decision.action {
            Some(crate::input::EncoderAction::MenuStep(direction)) => {
                let step = if direction > 0 { 1 } else { 7 };
                let rows = i32::try_from(crate::menu::MENU_ROWS).unwrap_or(1);
                crate::menu::with_state_mut(|menu| {
                    menu.selection = (menu.selection + step).rem_euclid(rows);
                });
                crate::status::set_status("");
                crate::ui::minimod_repaint_screen();
            }
            Some(crate::input::EncoderAction::SynthKindStep(direction)) => {
                let kind = crate::menu::step_bass_kind(direction);
                crate::backend::bass::preview_set_kind(kind);
                let knobs = crate::menu::bass_knobs();
                crate::backend::bass::preview_set_params(knobs[0], knobs[1], knobs[2]);
                crate::ui::minimod_repaint_screen();
            }
            None => {}
        }
        decision.flow
    }

    fn on_button(&mut self, event: ButtonEvent) -> Flow {
        #[cfg(any(feature = "dictation", feature = "dictation-rknn"))]
        if crate::speech::on_button(&event) {
            return Flow::Consume;
        }
        crate::recent::on_button(event);
        // Finish a job first: a completion that switched the page to chop mode
        // arms and applies its restore inside this callback.
        crate::ui::note_button(&event);
        crate::overlay::poll_offpage();
        crate::spectral::on_button(&event);
        crate::ui::restore_pending_chop(&event);
        let decision = crate::input::button(&mut self.input, event);
        crate::midi::set_on(i32::from(decision.levels));
        if let Some(action) = decision.action {
            let release_pads = crate::ui::apply_action(action, event, crate::input::last_pad());
            if release_pads {
                crate::input::cancel_forwarding();
                while let Some(release) = crate::input::take_forwarded_release() {
                    // SAFETY: the stored input and source are from this press;
                    // note-off is sent after its release owner is removed.
                    unsafe {
                        mpc_sample_sdk::midi::send_note_off(
                            release.input,
                            release.source,
                            release.pad,
                        );
                    }
                    crate::input::finish_forced_release(&release);
                }
            }
        }
        decision.flow
    }

    fn on_cc(&mut self, event: CcEvent) -> Flow {
        crate::ui::cancel_chop_restore();
        let decision = crate::input::cc(event);
        match decision.action {
            Some(crate::input::CcAction::Master(value)) => {
                crate::backend::looper::looper_set_master(value);
                crate::settings::save(crate::backend::looper::looper_master());
            }
            Some(crate::input::CcAction::BassKnob(index, amount)) => {
                let mut knobs = crate::menu::bass_knobs();
                knobs[index] = amount;
                crate::menu::set_bass_knobs(knobs);
                crate::backend::bass::preview_set_params(knobs[0], knobs[1], knobs[2]);
                crate::ui::minimod_repaint_screen();
            }
            None => {}
        }
        decision.flow
    }

    fn on_pitch(&mut self, applied: i32) -> i32 {
        crate::levels::apply(applied)
    }

    fn on_label(&mut self, label: &core::ffi::CStr) -> Option<&'static core::ffi::CStr> {
        crate::ui::label(label)
    }

    fn on_text_assignment(
        &mut self,
        dest: usize,
        label: &core::ffi::CStr,
    ) -> Option<&'static core::ffi::CStr> {
        #[cfg(any(feature = "dictation", feature = "dictation-rknn"))]
        crate::speech::observe_assignment(dest, label);
        #[cfg(not(any(feature = "dictation", feature = "dictation-rknn")))]
        let _ = dest;
        crate::ui::label(label)
    }

    fn on_repaint(&mut self, _component: usize) {
        #[cfg(any(feature = "dictation", feature = "dictation-rknn"))]
        {
            crate::speech::observe_repaint(_component);
            crate::speech::on_repaint();
        }
    }

    fn on_paint(&mut self, graphics: &mut mpc_sample_sdk::draw::Graphics, event: PaintEvent) {
        crate::overlay::paint(*graphics, event);
        crate::menu::follow_preview();
    }

    fn input(&self) -> InputHooks {
        InputHooks {
            pad: Some(live_pad),
            pad_complete: Some(live_pad_complete),
            surface: Some(live_surface),
            note: Some(live_note),
        }
    }

    fn audio(&self) -> AudioHooks {
        AudioHooks {
            output: Some(crate::backend::audio::output),
            input_interleaved: Some(audio_input_interleaved),
            input_planar: Some(crate::backend::audio::input_planar),
        }
    }
}

fn audio_input_interleaved(samples: &[i32], channels: usize, rate: u32) {
    crate::backend::audio::input_interleaved(samples, channels, rate);
    #[cfg(any(feature = "dictation", feature = "dictation-rknn"))]
    crate::speech::input_interleaved(samples, channels, rate);
}

/// Handle physical pad events without the UI module lock.
fn live_pad(event: PadEvent) -> PadResponse {
    crate::ui::request_repaint();
    let decision = crate::input::pad(event);
    if let Some(pad) = decision.looper_pad {
        let _ = crate::backend::looper::looper_pad(
            pad,
            i32::from(decision.shift),
            i32::from(decision.erase),
        );
    }
    PadResponse {
        flow: decision.flow,
        token: decision.token,
    }
}

/// Finish press ownership after the SDK has forwarded the original press.
fn live_pad_complete(event: PadEvent, token: u64, sent: bool) {
    if crate::input::finish_pad_press(event, token, sent) {
        // SAFETY: this completion runs inside the still-live native pad callback.
        unsafe { mpc_sample_sdk::midi::send_note_off(event.input, event.source, event.pad) };
    }
}

/// Handle surface pad events without taking the UI module lock.
fn live_surface(event: SurfaceEvent) -> Flow {
    let decision = crate::input::surface(event);
    if let Some(note) = decision.preview {
        crate::backend::bass::preview_note(note.pad, note.velocity);
        crate::ui::request_repaint();
    }
    decision.flow
}

/// Handle external MIDI notes without taking the UI module lock.
///
/// The replay state is atomic and the press/release pairing is a bounded
/// ledger, so a release can never be forwarded past a press the mod consumed.
fn live_note(event: &mut NoteEvent<'_>) -> Flow {
    crate::midi::note(event, crate::input::last_pad())
}

mpc_sample_sdk::export_mod!(Minimod);
