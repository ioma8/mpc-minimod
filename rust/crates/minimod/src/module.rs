//! The mod entry point: bind the feature policy to the SDK.
//!
//! Every hook, the firmware image and the audio taps belong to the SDK.  This
//! type only decides what the features do with the events.

use mpc_sample_sdk::{
    AudioHooks, ButtonEvent, ButtonFlow, CcEvent, CcFlow, EncoderEvent, EncoderFlow, Mod,
    NoteEvent, NoteFlow, PadEvent, PadFlow, PaintEvent, SurfaceEvent, SurfaceFlow,
};

/// The mod.  Its state lives in the feature modules, as small process statics.
#[derive(Default)]
pub struct Minimod;

impl Mod for Minimod {
    fn on_start(&mut self) {
        // The preview mixes from the note hooks, so it exists only when both
        // were installed.
        crate::backend::bass::set_available(mpc_sample_sdk::sites::notes_ready());
        crate::settings::load_into_mod();
        // One pump for the life of the process: the looper's animation and the
        // single frame a finished worker asks for both go through it, so no
        // worker ever calls the app's UI from its own thread.
        crate::paint::start_repaint_pump(crate::ui::pump_component);
    }

    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        // Finish a job or a chop that no paint will reach.
        crate::overlay::poll_offpage();
        crate::ui::cancel_chop_restore();
        crate::input::pad(event)
    }

    fn on_surface(&mut self, event: SurfaceEvent) -> SurfaceFlow {
        crate::input::surface(event)
    }

    fn on_encoder(&mut self, event: EncoderEvent) -> EncoderFlow {
        crate::ui::cancel_chop_restore();
        crate::input::encoder(event)
    }

    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        crate::overlay::poll_offpage();
        crate::spectral::on_button(&event);
        crate::ui::button(event)
    }

    fn on_cc(&mut self, event: CcEvent) -> CcFlow {
        crate::ui::cancel_chop_restore();
        crate::cc::control(event)
    }

    fn on_note(&mut self, event: &mut NoteEvent) -> NoteFlow {
        crate::midi::note(event)
    }

    fn on_label(&mut self, label: &core::ffi::CStr) -> Option<&'static core::ffi::CStr> {
        crate::ui::label(label)
    }

    fn on_pitch(&mut self, applied: i32) -> i32 {
        crate::levels::apply(applied)
    }

    fn on_paint(&mut self, graphics: &mut mpc_sample_sdk::draw::Graphics, event: PaintEvent) {
        crate::overlay::paint(*graphics, event);
        crate::menu::follow_preview();
    }

    fn audio(&self) -> AudioHooks {
        AudioHooks {
            output: Some(crate::backend::audio::output),
            input_interleaved: Some(crate::backend::audio::input_interleaved),
            input_planar: Some(crate::backend::audio::input_planar),
        }
    }
}

mpc_sample_sdk::export_mod!(Minimod);
