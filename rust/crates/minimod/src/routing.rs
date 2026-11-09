//! What a control-change event means while the mod owns a screen.
//!
//! The rules are a precedence ladder: the master volume always drives the loops
//! (and falls through to the app's own mix), an open looper owns everything else
//! while it is up, the bass control page takes the three knobs, and a modal menu
//! swallows the rest so the app never edits the sample behind it.
//!
//! The ladder is pure; the caller applies what it decides.

use core::sync::atomic::{AtomicI32, Ordering};

/// Master volume.
pub const CC_MASTER: u16 = 0x13;
/// Knob 1.
pub const CC_KNOB1: u16 = 0x10;
/// Knob 2.
pub const CC_KNOB2: u16 = 0x11;
/// Knob 3.
pub const CC_KNOB3: u16 = 0x12;
/// The encoder.
pub const CC_DATA: u16 = 0x64;

/// One millisecond of trim, in frames at 44.1 kHz.
const FRAMES_PER_MS: i32 = 44;
/// The trim knob's range, in milliseconds either way.
const TRIM_LIMIT_MS: i32 = 400;

/// The last knob-1 value seen, so the trim follows the knob's movement rather
/// than its absolute position.
static KNOB1_LAST: AtomicI32 = AtomicI32::new(-1);

/// What the caller should do with the event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Route {
    /// Nothing for the mod: hand the event to the app.
    Pass,
    /// Drive the loops' master, then pass the event on.
    Master,
    /// Move the trim by this many frames.
    Trim(i32),
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
    /// Whether this event is the mod replaying its own command.
    pub replaying: bool,
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
    /// The trim already applied, in frames.
    pub trim_frames: i32,
}

/// Decide what to do with one control-change event.
#[must_use]
pub fn route(context: Context) -> Route {
    if context.replaying {
        return Route::Pass;
    }
    if context.control == CC_MASTER {
        return Route::Master;
    }
    if context.screens.looper {
        if context.control == CC_KNOB1 {
            return trim(context.value).map_or(Route::Consume, Route::Trim);
        }
        // The looper owns the other controls while it is open.
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

/// The trim the knob's movement asks for, or `None` when it did not move.
fn trim(value: u32) -> Option<i32> {
    let Ok(value) = i32::try_from(value) else {
        return None;
    };
    let last = KNOB1_LAST.load(Ordering::Acquire);
    if last < 0 {
        KNOB1_LAST.store(value, Ordering::Release);
        return None;
    }
    if value == last {
        return None;
    }
    KNOB1_LAST.store(value, Ordering::Release);
    let frames = (value - last) * FRAMES_PER_MS;
    let existing = TRIM.load(Ordering::Acquire);
    let updated = existing.saturating_add(frames);
    Some(updated.clamp(
        -TRIM_LIMIT_MS * FRAMES_PER_MS,
        TRIM_LIMIT_MS * FRAMES_PER_MS,
    ))
}

static TRIM: AtomicI32 = AtomicI32::new(0);

/// Record the trim the caller actually applied, so the next step starts there.
pub fn set_trim(frames: i32) {
    TRIM.store(frames, Ordering::Release);
}

/// The decision, as a `#[repr(C)]` record for the glue.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawRoute {
    /// 0 pass, 1 master, 2 trim, 3 bass, 4 consume.
    pub kind: i32,
    /// The trim in frames, or the knob index for a bass route.
    pub value: i32,
    /// The bass knob's value.
    pub amount: f32,
}

/// Decide what to do, for the C glue.
#[must_use]
pub extern "C" fn minimod_cc_route(
    replaying: i32,
    control: u32,
    value: u32,
    range: f32,
    looper_open: i32,
    menu_open: i32,
    bass_edit: i32,
    busy: i32,
    spectral_active: i32,
    trim_frames: i32,
) -> RawRoute {
    let context = Context {
        replaying: replaying != 0,
        control: u16::try_from(control).unwrap_or(0xffff),
        value,
        range,
        screens: Screens {
            looper: looper_open != 0,
            menu: menu_open != 0,
            bass_edit: bass_edit != 0,
        },
        jobs: Jobs {
            busy: busy != 0,
            spectral: spectral_active != 0,
        },
        trim_frames,
    };
    set_trim(context.trim_frames);
    match route(context) {
        Route::Pass => RawRoute {
            kind: 0,
            value: 0,
            amount: 0.0,
        },
        Route::Master => RawRoute {
            kind: 1,
            value: 0,
            amount: 0.0,
        },
        Route::Trim(frames) => RawRoute {
            kind: 2,
            value: frames,
            amount: 0.0,
        },
        Route::Bass(knob, amount) => RawRoute {
            kind: 3,
            value: i32::try_from(knob).unwrap_or(0),
            amount,
        },
        Route::Consume => RawRoute {
            kind: 4,
            value: 0,
            amount: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, Jobs, Route, Screens, route as decide};

    fn plain() -> Context {
        Context {
            replaying: false,
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
            trim_frames: 0,
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
    fn a_replayed_command_never_routes_twice() {
        assert_eq!(
            decide(Context {
                replaying: true,
                control: 0x13,
                ..with_screens(true, false, false)
            }),
            Route::Pass
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
