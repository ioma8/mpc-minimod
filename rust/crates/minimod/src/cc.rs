//! What a control-change event does before it reaches the app.
//!
//! The routing decision is [`crate::routing`]'s; this acts on it: the master
//! volume belongs to the looper, the three knobs belong to the bass page, and
//! everything else is either swallowed (a modal screen owns the controls) or
//! handed to the app.

use crate::backend::bass::preview_set_params;
use crate::backend::looper;
use crate::clock;
use crate::routing;
use mpc_sample_sdk::state::Global;
use mpc_sample_sdk::{CcEvent, CcFlow};

/// How often the settings file may be written, in milliseconds.
const SAVE_EVERY_MS: i64 = 5_000;

/// The feature callbacks the routing uses.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Redraw the menu.
    pub repaint: Option<fn()>,
    /// Write the settings file.
    pub save: Option<fn()>,
}

/// The state the routing reads.
#[derive(Clone, Copy)]
pub struct Context {
    /// The raw value.
    pub value: u32,
    /// The controller's range.
    pub range: f32,
    /// The looper view.
    pub looper_open: i32,
    /// The effects menu.
    pub menu_open: i32,
    /// The bass control page.
    pub bass_edit: i32,
    /// A job is running.
    pub busy: i32,
    /// A spectral chop is running.
    pub spectral_active: i32,
    /// The three bass knob values.
    pub knobs: [f32; 3],
}

/// What the controller should apply.
#[derive(Clone, Copy, Default)]
pub struct Outcome {
    /// 1 when the event must not reach the app.
    pub consumed: i32,
    /// The three knob values after this event.
    pub knobs: [f32; 3],
    /// Whether `knobs` changed.
    pub knobs_changed: i32,
}

/// When the settings were last written.
static SAVED_AT: Global<i64> = Global::new(0);
/// Handle one control-change event.
#[must_use]
pub fn handle(context: Context, control: u32, hooks: Hooks) -> Outcome {
    let mut outcome = Outcome {
        consumed: 0,
        knobs: context.knobs,
        knobs_changed: 0,
    };
    let ladder = routing::Context {
        control: u16::try_from(control).unwrap_or(0xffff),
        value: context.value,
        range: context.range,
        screens: routing::Screens {
            looper: context.looper_open != 0,
            menu: context.menu_open != 0,
            bass_edit: context.bass_edit != 0,
        },
        jobs: routing::Jobs {
            busy: context.busy != 0,
            spectral: context.spectral_active != 0,
        },
    };
    match routing::route(ladder) {
        // The master volume drives the loops, then the app's own mix.
        routing::Route::Master => {
            looper::looper_set_master(context.value);
            save_throttled(hooks);
            // A modal screen still swallows the event after the master update;
            // the old fallback did this for every non-DATA control.
            if context.menu_open != 0 || context.spectral_active != 0 {
                outcome.consumed = 1;
            }
        }
        // The bass control page owns the three knobs.
        routing::Route::Bass(index, amount) => {
            let index = index.min(2);
            let mut knobs = context.knobs;
            knobs[index] = amount;
            outcome.knobs = knobs;
            outcome.knobs_changed = 1;
            // `control` applies the knobs once, after this ladder returns.
            if let Some(repaint) = hooks.repaint {
                repaint();
            }
            outcome.consumed = 1;
            return outcome;
        }
        // A modal screen keeps the app away from the sample.
        routing::Route::Consume => {
            outcome.consumed = 1;
            return outcome;
        }
        // Nothing for the mod.
        routing::Route::Pass => {}
    }
    outcome
}

/// Write the settings, at most once every few seconds.
fn save_throttled(hooks: Hooks) {
    let now = clock::now_ms();
    // SAFETY: `SAVED_AT` is owned by the app's UI thread.
    let saved = unsafe { SAVED_AT.with(|saved| *saved) };
    if now <= saved + SAVE_EVERY_MS {
        return;
    }
    // SAFETY: `SAVED_AT` is owned by the app's UI thread.
    unsafe { SAVED_AT.with_mut(|saved| *saved = now) };
    if let Some(save) = hooks.save {
        save();
    }
}

// The feature owns the screens and the key flags; it builds the context and
// applies what comes back.
/// What the routing needs to know about the mod's screens, right now.
fn context() -> Context {
    let menu = crate::menu::state();
    let knobs = crate::menu::bass_knobs();
    Context {
        value: 0,
        range: 0.0,
        looper_open: looper::looper_is_open(),
        menu_open: menu.open,
        bass_edit: menu.bass_edit,
        busy: menu.busy,
        spectral_active: i32::from(crate::spectral::is_active()),
        knobs,
    }
}

/// Write the master level to the settings file.
pub(crate) fn save() {
    crate::settings::save(looper::looper_master());
}

/// A control change: run the routing ladder and tell the SDK whether to forward.
pub fn control(event: CcEvent) -> CcFlow {
    let mut context = context();
    context.value = event.value;
    context.range = event.range;
    let hooks = Hooks {
        repaint: Some(crate::ui::minimod_repaint_screen),
        save: Some(save),
    };
    let outcome = handle(context, event.control, hooks);
    if outcome.knobs_changed != 0 {
        crate::menu::set_bass_knobs(outcome.knobs);
        preview_set_params(outcome.knobs[0], outcome.knobs[1], outcome.knobs[2]);
    }
    if outcome.consumed != 0 {
        CcFlow::Consume
    } else {
        CcFlow::Forward
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, Hooks, handle};

    fn context() -> Context {
        Context {
            value: 64,
            range: 127.0,
            looper_open: 0,
            menu_open: 0,
            bass_edit: 0,
            busy: 0,
            spectral_active: 0,
            knobs: [0.5, 0.5, 0.5],
        }
    }

    fn hooks() -> Hooks {
        Hooks {
            repaint: None,
            save: None,
        }
    }

    #[test]
    fn an_ordinary_knob_reaches_the_app() {
        // no callbacks are set, so nothing is invoked.
        let outcome = handle(context(), 0x20, hooks());
        assert_eq!(outcome.consumed, 0);
    }

    #[test]
    fn a_modal_screen_swallows_the_knobs_but_not_data() {
        let modal = Context {
            menu_open: 1,
            ..context()
        };
        // no callbacks are set.
        let outcome = handle(modal, 0x20, hooks());
        assert_eq!(outcome.consumed, 1);
        // no callbacks are set.
        let data = handle(modal, 0x64, hooks());
        assert_eq!(data.consumed, 0);
    }
}
