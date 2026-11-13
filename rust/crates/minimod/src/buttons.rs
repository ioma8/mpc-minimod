//! The button dispatcher: which key does what while the mod owns a screen.
//!
//! The app sends one event per press and release.  The mod either swallows the
//! event, changes its own state, or hands it back to the app.  The decision is a
//! pure function of the event and the current state; the caller applies the
//! result, because the state itself belongs to the caller.

pub use mpc_sample_sdk::event::button::{
    B3 as KEY_B3, DATA as KEY_DATA, ERASE as KEY_ERASE, LEVELS as KEY_LEVELS, MINUS as KEY_MINUS,
    PLUS as KEY_PLUS, PRESSED, SHIFT as KEY_SHIFT,
};

/// The Sample screen.
pub const KEY_SAMPLE: u32 = 0x06;
/// The first soft key (the trim screen).
pub const KEY_SCREEN1: u32 = 0x0c;
/// The second soft key.
pub const KEY_SCREEN2: u32 = 0x0d;
/// The fourth soft key.
pub const KEY_SCREEN4: u32 = 0x0f;

/// Whether a key leaves the trim screen, so the trim flag must be cleared.
///
/// The first soft key is the trim screen. The third soft key is [`KEY_B3`],
/// which the trim screen uses for the cycle; it is not a navigation key.
#[must_use]
pub const fn leaves_trim_screen(code: u32) -> bool {
    matches!(code, KEY_SAMPLE | KEY_SCREEN2 | KEY_SCREEN4)
}

/// What the caller should do with the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing for the mod: hand the event to the app.
    Forward,
    /// Swallowed.
    Consume,
    /// Track SHIFT (the value is the new state).
    Shift(bool),
    /// Track ERASE while the looper owns the pads.
    Erase(bool),
    /// The looper's own button handling.
    Looper,
    /// Open the effects menu.
    MenuOpen,
    /// Close it.
    MenuClose,
    /// Run the selected menu row.
    MenuActivate,
    /// Start the trim cycle.
    Autotrim,
    /// Move the 16 Levels row by this many octaves.
    BumpOctave(i32),
    /// PLUS and MINUS together.
    ResetPitch,
    /// Move the synth preview by this many 8-semitone steps.
    BumpSynthOctave(i32),
    /// PLUS and MINUS together on the synth page.
    ResetSynthPitch,
    /// Toggle 16 Levels.
    ToggleLevels,
    /// The encoder press was consumed by the menu.
    MenuEncoder,
}

/// The keys the user is holding.
#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    /// SHIFT.
    pub shift: bool,
}

/// The 16 Levels octave keys, held.
#[derive(Clone, Copy, Debug, Default)]
pub struct OctaveKeys {
    /// PLUS.
    pub plus: bool,
    /// MINUS.
    pub minus: bool,
}

/// The screens that are up.
#[derive(Clone, Copy, Debug, Default)]
pub struct Screens {
    /// The mod's own screens.
    pub modals: Modals,
    /// The looper view.
    pub looper: bool,
    /// The 16 Levels screen.
    pub levels: bool,
    /// The synth control page.
    pub bass_edit: bool,
}

/// The mod's modal screens.
#[derive(Clone, Copy, Debug, Default)]
pub struct Modals {
    /// The effects menu.
    pub menu: bool,
    /// A spectral chop is running.
    pub spectral: bool,
}

/// Everything the decision reads.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The key.
    pub code: u32,
    /// The value: `PRESSED` is a press.
    pub down: u32,
    /// The keys being held.
    pub held: Held,
    /// The octave keys being held.
    pub octave: OctaveKeys,
    /// The screens that are up.
    pub screens: Screens,
    /// Whether the Sample page is visible.
    pub page_visible: bool,
    /// Whether the app's trim screen is showing (the B3 label is `Filter`).
    pub trim_screen: bool,
    /// Whether the Sample page is in chop mode.
    pub chop_mode: bool,
}

/// Whether the event is a press.
#[must_use]
pub const fn pressed(down: u32) -> bool {
    down == PRESSED
}

/// Decide what to do with one button event.
#[must_use]
pub fn dispatch(context: Context) -> Action {
    let press = pressed(context.down);
    // The looper owns every key but SHIFT and ERASE, which it tracks itself.
    if context.screens.looper {
        return match context.code {
            KEY_SHIFT => Action::Shift(press),
            KEY_ERASE => Action::Erase(press),
            _ => Action::Looper,
        };
    }
    // The encoder drives the menu, and only the menu.
    if context.code == KEY_DATA {
        return encoder(context, press);
    }
    if context.code == KEY_SHIFT {
        return Action::Shift(press);
    }
    // A release belongs to the mod if its press was consumed, even if the
    // user changed modes or started holding SHIFT in the meantime.
    if !press
        && ((context.code == KEY_PLUS && context.octave.plus)
            || (context.code == KEY_MINUS && context.octave.minus))
    {
        return Action::BumpOctave(0);
    }
    // The synth page is modal too, but PLUS/MINUS shift its chromatic pads by
    // the same eight-semitone steps as 16 Levels.
    if (context.code == KEY_PLUS || context.code == KEY_MINUS) && context.screens.bass_edit {
        let plus = context.code == KEY_PLUS;
        if !press {
            return Action::BumpSynthOctave(0);
        }
        if if plus {
            context.octave.minus
        } else {
            context.octave.plus
        } {
            return Action::ResetSynthPitch;
        }
        return Action::BumpSynthOctave(if plus { 1 } else { -1 });
    }
    // A modal screen swallows the rest.
    if context.screens.modals.menu || context.screens.modals.spectral {
        return Action::Consume;
    }
    // The release matters too: it is what lets the next press count again.
    // AutoTrim edits the pad the user pressed last, so it is only armed on the
    // Sample page where that pad is the one being looked at.
    if context.code == KEY_B3
        && context.held.shift
        && context.page_visible
        && context.trim_screen
        && !context.chop_mode
    {
        return Action::Autotrim;
    }
    if (context.code == KEY_PLUS || context.code == KEY_MINUS) && context.screens.levels {
        let plus = context.code == KEY_PLUS;
        if !press {
            return Action::Forward;
        }
        if context.held.shift {
            return Action::Forward;
        }
        // Both keys together reset the row instead of moving it.
        if if plus {
            context.octave.minus
        } else {
            context.octave.plus
        } {
            return Action::ResetPitch;
        }
        return Action::BumpOctave(if plus { 1 } else { -1 });
    }
    if context.code == KEY_LEVELS
        && press
        && !context.held.shift
        && context.page_visible
        && !context.chop_mode
    {
        return Action::ToggleLevels;
    }
    Action::Forward
}

/// The encoder's own ladder.
const fn encoder(context: Context, press: bool) -> Action {
    if context.screens.modals.spectral {
        return Action::Consume;
    }
    if press && context.held.shift && context.page_visible {
        return if context.screens.modals.menu {
            Action::MenuClose
        } else {
            Action::MenuOpen
        };
    }
    if press && context.screens.modals.menu {
        return Action::MenuActivate;
    }
    if context.screens.modals.menu {
        return Action::MenuEncoder;
    }
    if context.held.shift && context.page_visible {
        return Action::Consume;
    }
    Action::Forward
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Context, Held, KEY_B3, KEY_DATA, KEY_LEVELS, KEY_MINUS, KEY_PLUS, KEY_SHIFT,
        Modals, OctaveKeys, PRESSED, Screens, dispatch,
    };

    fn base() -> Context {
        Context {
            code: 0,
            down: 0,
            held: Held::default(),
            octave: OctaveKeys::default(),
            screens: Screens::default(),
            page_visible: true,
            trim_screen: false,
            chop_mode: false,
        }
    }

    #[test]
    fn the_looper_owns_the_pads_but_not_shift() {
        let context = Context {
            screens: Screens {
                looper: true,
                ..Screens::default()
            },
            code: KEY_SHIFT,
            down: PRESSED,
            ..base()
        };
        assert_eq!(dispatch(context), Action::Shift(true));
        let context = Context {
            screens: Screens {
                looper: true,
                ..Screens::default()
            },
            code: 0x21,
            down: PRESSED,
            ..base()
        };
        assert_eq!(dispatch(context), Action::Looper);
    }

    #[test]
    fn shift_and_the_encoder_open_the_menu() {
        let context = Context {
            code: KEY_DATA,
            down: PRESSED,
            held: Held { shift: true },
            ..base()
        };
        assert_eq!(dispatch(context), Action::MenuOpen);
    }

    #[test]
    fn shift_and_the_encoder_close_an_open_menu_without_activating_a_row() {
        let context = Context {
            code: KEY_DATA,
            down: PRESSED,
            held: Held { shift: true },
            screens: Screens {
                modals: Modals {
                    menu: true,
                    ..Modals::default()
                },
                ..Screens::default()
            },
            ..base()
        };
        assert_eq!(dispatch(context), Action::MenuClose);
    }

    #[test]
    fn an_open_menu_takes_the_encoder_and_swallows_the_rest() {
        let context = Context {
            code: KEY_DATA,
            down: PRESSED,
            screens: Screens {
                modals: Modals {
                    menu: true,
                    ..Modals::default()
                },
                ..Screens::default()
            },
            ..base()
        };
        assert_eq!(dispatch(context), Action::MenuActivate);
        let context = Context {
            code: 0x21,
            down: PRESSED,
            screens: Screens {
                modals: Modals {
                    menu: true,
                    ..Modals::default()
                },
                ..Screens::default()
            },
            ..base()
        };
        assert_eq!(dispatch(context), Action::Consume);
    }

    #[test]
    fn sixteen_levels_toggles_only_on_a_press() {
        let context = Context {
            code: KEY_LEVELS,
            down: PRESSED,
            ..base()
        };
        assert_eq!(dispatch(context), Action::ToggleLevels);
        let context = Context {
            code: KEY_LEVELS,
            down: 0,
            ..base()
        };
        assert_eq!(dispatch(context), Action::Forward);
    }

    #[test]
    fn releasing_an_octave_key_clears_it_without_moving() {
        let context = Context {
            code: KEY_PLUS,
            down: PRESSED,
            screens: Screens {
                levels: true,
                ..Screens::default()
            },
            ..base()
        };
        assert_eq!(dispatch(context), Action::BumpOctave(1));
        let context = Context {
            code: KEY_PLUS,
            down: 0,
            octave: OctaveKeys {
                plus: true,
                minus: false,
            },
            screens: Screens {
                levels: true,
                ..Screens::default()
            },
            ..base()
        };
        assert_eq!(dispatch(context), Action::BumpOctave(0));
        let context = Context {
            code: KEY_MINUS,
            down: PRESSED,
            screens: Screens {
                levels: true,
                ..Screens::default()
            },
            ..base()
        };
        assert_eq!(dispatch(context), Action::BumpOctave(-1));
    }

    #[test]
    fn shift_shortcuts_are_forwarded_without_changing_levels() {
        let mut context = Context {
            code: KEY_LEVELS,
            down: PRESSED,
            held: Held { shift: true },
            ..base()
        };
        assert_eq!(dispatch(context), Action::Forward);
        context.screens.levels = true;
        for code in [KEY_PLUS, KEY_MINUS] {
            context.code = code;
            assert_eq!(dispatch(context), Action::Forward);
            context.down = 0;
            context.held.shift = false;
            assert_eq!(dispatch(context), Action::Forward);
            context.down = PRESSED;
            context.held.shift = true;
        }
        // A press already owned by the mod must still have its release consumed
        // if the user holds SHIFT or leaves 16 Levels before releasing it.
        context.code = KEY_PLUS;
        context.down = 0;
        context.octave.plus = true;
        context.screens.levels = false;
        assert_eq!(dispatch(context), Action::BumpOctave(0));
    }

    #[test]
    fn levels_press_does_not_change_mod_state_when_chop_is_open() {
        let context = Context {
            code: KEY_LEVELS,
            down: PRESSED,
            page_visible: true,
            chop_mode: true,
            ..base()
        };
        assert_eq!(dispatch(context), Action::Forward);
    }

    #[test]
    fn everything_else_reaches_the_app() {
        let context = Context {
            code: 0x21,
            down: PRESSED,
            ..base()
        };
        assert_eq!(dispatch(context), Action::Forward);
    }

    #[test]
    fn both_edges_of_shift_b3_reach_the_trim_cycle() {
        let mut context = Context {
            code: KEY_B3,
            down: PRESSED,
            held: Held { shift: true },
            octave: OctaveKeys::default(),
            screens: Screens::default(),
            page_visible: true,
            trim_screen: true,
            chop_mode: false,
        };
        assert_eq!(dispatch(context), Action::Autotrim);
        context.down = 0;
        assert_eq!(dispatch(context), Action::Autotrim);
        context.chop_mode = true;
        assert_eq!(dispatch(context), Action::Forward);
        context.chop_mode = false;
        // Off the trim screen the key belongs to the app.
        context.page_visible = false;
        assert_eq!(dispatch(context), Action::Forward);
        context.page_visible = true;
        context.trim_screen = false;
        assert_eq!(dispatch(context), Action::Forward);
    }

    #[test]
    fn the_synth_page_takes_plus_and_minus_in_eight_semitone_steps() {
        let mut context = Context {
            code: KEY_PLUS,
            down: PRESSED,
            screens: Screens {
                modals: Modals {
                    menu: true,
                    ..Modals::default()
                },
                bass_edit: true,
                ..Screens::default()
            },
            ..base()
        };
        assert_eq!(dispatch(context), Action::BumpSynthOctave(1));
        context.code = KEY_MINUS;
        assert_eq!(dispatch(context), Action::BumpSynthOctave(-1));
        context.octave.plus = true;
        assert_eq!(dispatch(context), Action::ResetSynthPitch);
        context.octave.plus = false;
        context.down = 0;
        assert_eq!(dispatch(context), Action::BumpSynthOctave(0));
    }
}
