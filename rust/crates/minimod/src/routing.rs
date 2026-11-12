//! What a control-change event means while the mod owns a screen.
//!
//! The rules are a precedence ladder: the master volume always drives the loops
//! (and falls through to the app's own mix), an open looper owns everything else
//! while it is up, the bass control page takes the three knobs, and a modal menu
//! swallows the rest so the app never edits the sample behind it.
//!
//! The ladder is pure; the caller applies what it decides.

pub use mpc_sample_sdk::event::cc::{
    DATA as CC_DATA, KNOB1 as CC_KNOB1, KNOB2 as CC_KNOB2, KNOB3 as CC_KNOB3, MASTER as CC_MASTER,
};

/// What the caller should do with the event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Route {
    /// Nothing for the mod: hand the event to the app.
    Pass,
    /// Drive the loops' master, then pass the event on.
    Master,
    /// Set this bass knob (0..2) to this value.
    Bass(usize, f32),
    /// Swallow the event.
    Consume,
}

/// Which of the mod's screens are up.
#[derive(Clone, Copy, Debug)]
pub struct Screens {
    /// The looper view.
    pub looper: bool,
    /// The effects menu.
    pub menu: bool,
    /// The bass control page.
    pub bass_edit: bool,
}

/// What the mod is busy with.
#[derive(Clone, Copy, Debug)]
pub struct Jobs {
    /// A render or extract is running.
    pub busy: bool,
    /// A spectral chop is running.
    pub spectral: bool,
}

/// Everything the ladder looks at.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The controller number.
    pub control: u16,
    /// The raw value.
    pub value: u32,
    /// The knob's range, for controllers that report one.
    pub range: f32,
    /// The screens that are up.
    pub screens: Screens,
    /// What is running.
    pub jobs: Jobs,
}

/// Decide what to do with one control-change event.
#[must_use]
pub fn route(context: Context) -> Route {
    if context.control == CC_MASTER {
        return Route::Master;
    }
    if context.screens.looper {
        // The looper owns the controls while it is open; its latency is the
        // device's own value.
        return Route::Consume;
    }
    if context.screens.menu && context.screens.bass_edit && !context.jobs.busy {
        let knob = match context.control {
            CC_KNOB1 => 0,
            CC_KNOB2 => 1,
            CC_KNOB3 => 2,
            _ => return pass_or_consume(context),
        };
        let scale = if context.range > 1.0 {
            context.range
        } else {
            127.0
        };
        let value = f32::from(u16::try_from(context.value).unwrap_or(u16::MAX)) / scale;
        return Route::Bass(knob, value.clamp(0.0, 1.0));
    }
    pass_or_consume(context)
}

/// With a modal screen up, only the encoder reaches the app.
fn pass_or_consume(context: Context) -> Route {
    if (context.screens.menu || context.jobs.spectral) && context.control != CC_DATA {
        return Route::Consume;
    }
    Route::Pass
}

#[cfg(test)]
mod tests {
    use super::{Context, Jobs, Route, Screens, route as decide};

    fn plain() -> Context {
        Context {
            control: 0x99,
            value: 0,
            range: 0.0,
            screens: Screens {
                looper: false,
                menu: false,
                bass_edit: false,
            },
            jobs: Jobs {
                busy: false,
                spectral: false,
            },
        }
    }

    fn with_screens(looper: bool, menu: bool, bass_edit: bool) -> Context {
        Context {
            screens: Screens {
                looper,
                menu,
                bass_edit,
            },
            ..plain()
        }
    }

    #[test]
    fn the_master_drives_the_loops_and_still_reaches_the_app() {
        assert_eq!(
            decide(Context {
                control: 0x13,
                ..plain()
            }),
            Route::Master
        );
    }

    #[test]
    fn the_open_looper_owns_everything_but_the_master() {
        assert_eq!(
            decide(Context {
                control: 0x11,
                ..with_screens(true, false, false)
            }),
            Route::Consume
        );
        assert_eq!(
            decide(Context {
                control: 0x13,
                ..with_screens(true, false, false)
            }),
            Route::Master
        );
    }

    #[test]
    fn the_bass_page_takes_the_three_knobs_and_scales_them() {
        assert_eq!(
            decide(Context {
                control: 0x11,
                value: 127,
                ..with_screens(false, true, true)
            }),
            Route::Bass(1, 1.0)
        );
        assert_eq!(
            decide(Context {
                control: 0x12,
                value: 64,
                ..with_screens(false, true, true)
            }),
            Route::Bass(2, 64.0 / 127.0)
        );
    }

    #[test]
    fn a_running_job_ignores_the_knobs() {
        let context = Context {
            control: 0x10,
            jobs: Jobs {
                busy: true,
                spectral: false,
            },
            ..with_screens(false, true, true)
        };
        assert_eq!(decide(context), Route::Consume);
    }

    #[test]
    fn a_modal_screen_swallows_everything_but_the_encoder() {
        assert_eq!(
            decide(Context {
                control: 0x10,
                ..with_screens(false, true, false)
            }),
            Route::Consume
        );
        assert_eq!(
            decide(Context {
                control: 0x64,
                ..with_screens(false, true, false)
            }),
            Route::Pass
        );
        let spectral = Context {
            control: 0x11,
            jobs: Jobs {
                busy: false,
                spectral: true,
            },
            ..plain()
        };
        assert_eq!(decide(spectral), Route::Consume);
    }
}
