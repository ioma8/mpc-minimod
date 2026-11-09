//! The mod entry point: bind the feature policy to the SDK.
//!
//! Every hook, the firmware image and the audio taps belong to the SDK.  This
//! type only decides what the features do with the events.

use mpc_sample_sdk::{
    ButtonEvent, ButtonFlow, CcEvent, CcFlow, EncoderEvent, EncoderFlow, Mod, NoteEvent, NoteFlow,
    PadEvent, PadFlow, PaintEvent, SurfaceEvent, SurfaceFlow,
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
    }

    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        // Finish a job or a chop that no paint will reach.
        crate::overlay::poll_offpage();
        crate::input::pad(event)
    }

    fn on_surface(&mut self, event: SurfaceEvent) -> SurfaceFlow {
        crate::input::surface(event)
    }

    fn on_encoder(&mut self, event: EncoderEvent) -> EncoderFlow {
        crate::input::encoder(event)
    }

    fn on_button(&mut self, event: ButtonEvent) -> ButtonFlow {
        crate::overlay::poll_offpage();
        crate::ui::button(event)
    }

    fn on_cc(&mut self, event: CcEvent) -> CcFlow {
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

    fn on_input_interleaved(&mut self, frames: &[i32], channels: usize, rate: u32) {
        crate::backend::audio::input_interleaved(frames, channels, rate);
    }

    fn on_input_planar(&mut self, channels: &[&[i32]], rate: u32) {
        crate::backend::audio::input_planar(channels, rate);
    }

    fn on_output(&mut self, frames: &mut [i32], info: &mpc_sample_sdk::audio::OutputInfo) {
        crate::backend::audio::output(frames, info);
    }
}

mpc_sample_sdk::export_mod!(Minimod);
