//! The button dispatcher: which key does what while the mod owns a screen.
//!
//! The app sends one event per press and release.  The mod either swallows the
//! event, changes its own state, or hands it back to the app.  The decision is a
//! pure function of the event and the current state; the caller applies the
//! result, because the state itself belongs to the caller.

/// Key codes the mod cares about.
pub const KEY_SHIFT: u32 = 0x31;
/// ERASE.
pub const KEY_ERASE: u32 = 0x09;
/// The encoder.
pub const KEY_DATA: u32 = 0x6f;
/// SHIFT + B3: the trim cycle.
pub const KEY_B3: u32 = 0x0e;
/// PLUS / MINUS: the 16 Levels octave.
pub const KEY_PLUS: u32 = 0x36;
/// MINUS.
pub const KEY_MINUS: u32 = 0x37;
/// 16 LEVELS.
pub const KEY_LEVELS: u32 = 0x28;
/// A press can be reported as the full travel value.
pub const PRESSED: u32 = 0x7f;

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
    // A modal screen swallows the rest.
    if context.screens.modals.menu || context.screens.modals.spectral {
        return Action::Consume;
    }
    // The release matters too: it is what lets the next press count again.
    if context.code == KEY_B3 && context.held.shift {
        return Action::Autotrim;
    }
    if (context.code == KEY_PLUS || context.code == KEY_MINUS) && context.screens.levels {
        let plus = context.code == KEY_PLUS;
        if !press {
            return Action::Consume;
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
    if context.code == KEY_LEVELS && press {
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
        Action, Context, Held, KEY_B3, KEY_DATA, KEY_LEVELS, KEY_SHIFT, Modals, OctaveKeys,
        PRESSED, Screens, dispatch,
    };

    fn base() -> Context {
        Context {
            code: 0,
            down: 0,
            held: Held::default(),
            octave: OctaveKeys::default(),
            screens: Screens::default(),
            page_visible: true,
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
        };
        assert_eq!(dispatch(context), Action::Autotrim);
        context.down = 0;
        assert_eq!(dispatch(context), Action::Autotrim);
    }
}
