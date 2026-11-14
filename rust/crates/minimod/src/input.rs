//! Device-control policy: own event history, decide who receives each event,
//! and return feature actions for [`crate::module::Minimod`] to dispatch.
//!
//! The SDK still decodes and forwards native events. Feature screens keep their
//! own state; this module reads their current modes to make one consistent
//! ownership decision across buttons, pads, surface events, encoder turns and CC.

use crate::buttons::{
    self, Action as ButtonAction, Context as ButtonContext, Held, Modals, OctaveKeys,
    Screens as ButtonScreens,
};
use crate::pad_ledger::{ForcedRelease, PadLedger};
use crate::routing::{self, Context as CcContext, Jobs, Route, Screens as CcScreens};
use core::sync::atomic::{AtomicI32, Ordering};
use mpc_sample_sdk::{ButtonEvent, CcEvent, EncoderEvent, Flow, PadAction, PadEvent, SurfaceEvent};

/// Pad history is independent of the UI module lock.
static PAD_LEDGER: PadLedger = PadLedger::new();
static LAST_PAD: AtomicI32 = AtomicI32::new(-1);

/// The UI-owned button and encoder history.
#[derive(Clone, Debug)]
pub(crate) struct State {
    data_target: usize,
    plus_held: bool,
    minus_held: bool,
    b3_down: bool,
}

impl State {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            data_target: 0,
            plus_held: false,
            minus_held: false,
            b3_down: false,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// Last physical pad whose press was forwarded to the app.
#[must_use]
pub(crate) fn last_pad() -> i32 {
    LAST_PAD.load(Ordering::Acquire)
}

/// Cancel in-flight forwards before closing the looper.
pub(crate) fn cancel_forwarding() {
    PAD_LEDGER.cancel_forwarding();
}

/// Claim an app-owned pad for a synthetic note-off.
pub(crate) fn take_forwarded_release() -> Option<ForcedRelease> {
    PAD_LEDGER.take_forwarded()
}

/// Complete a synthetic note-off claim.
pub(crate) fn finish_forced_release(claim: &ForcedRelease) {
    PAD_LEDGER.finish_forced(claim);
}

/// Complete a pad press after the native handler has returned.
pub(crate) fn finish_pad_press(event: PadEvent, token: u64, sent: bool) -> bool {
    usize::try_from(event.pad).is_ok_and(|pad| PAD_LEDGER.finish_press(pad, token, sent))
}

/// The feature modes the input policy observes for one event.
#[derive(Clone, Copy, Debug, Default)]
#[allow(clippy::struct_excessive_bools)] // Snapshot of independent feature modes for one event.
struct Modes {
    looper: bool,
    spectral: bool,
    menu: bool,
    bass_edit: bool,
    busy: bool,
    page_visible: bool,
    trim_screen: bool,
    chop_mode: bool,
    shift: bool,
    erase: bool,
}

/// A button decision and the feature action, if the event triggers one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ButtonDecision {
    pub(crate) flow: Flow,
    pub(crate) action: Option<ButtonAction>,
    /// The current firmware 16 Levels state, to synchronize MINIMOD's MIDI mode.
    pub(crate) levels: bool,
}

/// A pad decision; `looper_pad` is an action for the live looper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PadDecision {
    pub(crate) flow: Flow,
    pub(crate) looper_pad: Option<i32>,
    pub(crate) shift: bool,
    pub(crate) erase: bool,
    /// SDK completion token for a forwarded press.
    pub(crate) token: u64,
}

/// A bass preview note triggered by the surface pad stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreviewNote {
    pub(crate) pad: u32,
    pub(crate) velocity: f32,
}

/// A surface event decision; surface pad notes have their own policy from pads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SurfaceDecision {
    pub(crate) flow: Flow,
    pub(crate) preview: Option<PreviewNote>,
}

/// A menu action produced by an encoder turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EncoderAction {
    MenuStep(i32),
    SynthKindStep(i32),
}

/// An encoder flow decision and optional action for the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EncoderDecision {
    pub(crate) flow: Flow,
    pub(crate) action: Option<EncoderAction>,
}

/// An effect of a CC event after the ownership decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CcAction {
    Master(u32),
    BassKnob(usize, f32),
}

/// A CC flow decision and optional feature action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CcDecision {
    pub(crate) flow: Flow,
    pub(crate) action: Option<CcAction>,
}

/// Which pads the app can address.
const PADS: usize = 128;
const LOOPER_PADS: usize = crate::backend::looper::PADS;
const PREVIEW_PADS: usize = crate::backend::bass::PADS;

fn pad_index(pad: i32) -> Option<usize> {
    usize::try_from(pad).ok().filter(|index| *index < PADS)
}

/// Decide a button event and update only input-owned history.
#[must_use]
pub(crate) fn button(state: &mut State, event: ButtonEvent) -> ButtonDecision {
    track_screen_button(event);
    if event.surface != 0 {
        state.data_target = mpc_sample_sdk::event::data_target(event.surface);
    }
    button_with_modes(state, event, modes(), crate::midi::levels_active() != 0)
}

fn button_with_modes(
    state: &mut State,
    event: ButtonEvent,
    modes: Modes,
    levels: bool,
) -> ButtonDecision {
    let action = decide_button(state, event, modes, levels);
    let should_run = if action == ButtonAction::Autotrim {
        let first_press = event.pressed && !state.b3_down;
        if event.code == buttons::KEY_B3 {
            state.b3_down = event.pressed;
        }
        first_press
    } else {
        if event.code == buttons::KEY_B3 {
            state.b3_down = event.pressed;
        }
        true
    };
    track_owned_octave_key(state, event, action);
    ButtonDecision {
        flow: if modes.looper {
            Flow::Consume
        } else {
            button_flow(action)
        },
        action: should_run.then_some(action),
        levels,
    }
}

fn decide_button(state: &State, event: ButtonEvent, modes: Modes, levels: bool) -> ButtonAction {
    let context = ButtonContext {
        code: event.code,
        down: event.value,
        held: Held { shift: event.shift },
        octave: OctaveKeys {
            plus: state.plus_held,
            minus: state.minus_held,
        },
        screens: ButtonScreens {
            modals: Modals {
                menu: modes.menu,
                spectral: modes.spectral,
            },
            looper: modes.looper,
            levels,
            bass_edit: modes.bass_edit,
        },
        page_visible: modes.page_visible,
        trim_screen: modes.trim_screen,
        chop_mode: modes.chop_mode && matches!(event.code, buttons::KEY_B3 | buttons::KEY_LEVELS),
    };
    buttons::dispatch(context)
}

fn track_owned_octave_key(state: &mut State, event: ButtonEvent, action: ButtonAction) {
    let owns_press = matches!(
        action,
        ButtonAction::BumpOctave(_)
            | ButtonAction::ResetPitch
            | ButtonAction::BumpSynthOctave(_)
            | ButtonAction::ResetSynthPitch
    );
    if event.code == buttons::KEY_PLUS {
        if owns_press || !event.pressed {
            state.plus_held = event.pressed && owns_press;
        }
    } else if event.code == buttons::KEY_MINUS && (owns_press || !event.pressed) {
        state.minus_held = event.pressed && owns_press;
    }
}

const fn button_flow(action: ButtonAction) -> Flow {
    match action {
        ButtonAction::Forward | ButtonAction::Shift(_) | ButtonAction::ToggleLevels => {
            Flow::Forward
        }
        _ => Flow::Consume,
    }
}

/// Decide a pad edge on the independent live input path.
#[must_use]
pub(crate) fn pad(event: PadEvent) -> PadDecision {
    let bass_page = crate::menu::live_bass_page();
    let live = Modes {
        looper: crate::backend::looper::looper_is_open() != 0,
        spectral: crate::spectral::live_active(),
        menu: bass_page,
        bass_edit: bass_page,
        shift: mpc_sample_sdk::shift(),
        erase: mpc_sample_sdk::erase(),
        ..Modes::default()
    };
    route_pad(&PAD_LEDGER, &LAST_PAD, event, live)
}

fn route_pad(
    ledger: &PadLedger,
    last_pad: &AtomicI32,
    event: PadEvent,
    modes: Modes,
) -> PadDecision {
    let modal = (modes.looper && event.pad >= 0)
        || modes.spectral
        || (modes.menu
            && modes.bass_edit
            && usize::try_from(event.pad).is_ok_and(|pad| pad < PREVIEW_PADS));
    let mut flow = if modal { Flow::Consume } else { Flow::Forward };
    let mut token = 0;
    let mut accepted = true;
    if let Some(pad) = pad_index(event.pad) {
        match event.action {
            PadAction::Press => {
                if let Some(reserved) = ledger.press(pad, event.input, event.source, !modal) {
                    token = reserved;
                    if !modal {
                        last_pad.store(event.pad, Ordering::Release);
                    }
                } else {
                    flow = Flow::Consume;
                    accepted = false;
                }
            }
            PadAction::Release => flow = ledger.release(pad, event.input, event.source),
        }
    } else if event.action == PadAction::Press && !modal {
        last_pad.store(event.pad, Ordering::Release);
    }
    PadDecision {
        flow,
        looper_pad: if accepted {
            looper_pad(modes, event)
        } else {
            None
        },
        shift: modes.shift,
        erase: modes.erase,
        token,
    }
}

fn looper_pad(modes: Modes, event: PadEvent) -> Option<i32> {
    (modes.looper
        && event.action == PadAction::Press
        && usize::try_from(event.pad).is_ok_and(|pad| pad < LOOPER_PADS))
    .then_some(event.pad)
}

/// Decide a surface event independently from the app's pad press/release path.
#[must_use]
pub(crate) fn surface(event: SurfaceEvent) -> SurfaceDecision {
    let bass_page = crate::menu::live_bass_page();
    surface_with_modes(
        event,
        Modes {
            menu: bass_page,
            bass_edit: bass_page,
            ..Modes::default()
        },
    )
}

fn surface_with_modes(event: SurfaceEvent, modes: Modes) -> SurfaceDecision {
    if !modes.menu
        || !modes.bass_edit
        || !usize::try_from(event.code).is_ok_and(|pad| pad < PREVIEW_PADS)
        || !event.is_pad_event()
    {
        return SurfaceDecision {
            flow: Flow::Forward,
            preview: None,
        };
    }
    SurfaceDecision {
        flow: Flow::Consume,
        preview: event.is_pad_press().then_some(PreviewNote {
            pad: u32::try_from(event.code).unwrap_or(0),
            velocity: f32::from(event.velocity) / 127.0,
        }),
    }
}

/// Decide whether a DATA turn belongs to the menu.
#[must_use]
pub(crate) fn encoder(state: &State, event: EncoderEvent) -> EncoderDecision {
    encoder_with_modes(state, event, modes())
}

fn encoder_with_modes(state: &State, event: EncoderEvent, modes: Modes) -> EncoderDecision {
    if event.surface != state.data_target || (!modes.menu && !modes.spectral) {
        return EncoderDecision {
            flow: Flow::Forward,
            action: None,
        };
    }
    let action = if modes.menu && !modes.busy && event.delta != 0 {
        let direction = if event.delta > 0 { 1 } else { -1 };
        Some(if modes.bass_edit {
            EncoderAction::SynthKindStep(direction)
        } else {
            EncoderAction::MenuStep(direction)
        })
    } else {
        None
    };
    EncoderDecision {
        flow: Flow::Consume,
        action,
    }
}

/// Decide what a CC should do while the menu or Live Looper owns input.
#[must_use]
pub(crate) fn cc(event: CcEvent) -> CcDecision {
    decide_cc(event, modes())
}

fn decide_cc(event: CcEvent, modes: Modes) -> CcDecision {
    let route = routing::route(CcContext {
        control: u16::try_from(event.control).unwrap_or(0xffff),
        value: event.value,
        range: event.range,
        screens: CcScreens {
            looper: modes.looper,
            menu: modes.menu,
            bass_edit: modes.bass_edit,
        },
        jobs: Jobs {
            busy: modes.busy,
            spectral: modes.spectral,
        },
    });
    let (flow, action) = match route {
        Route::Master => (
            if modes.menu || modes.spectral {
                Flow::Consume
            } else {
                Flow::Forward
            },
            Some(CcAction::Master(event.value)),
        ),
        Route::Bass(index, amount) => (
            Flow::Consume,
            Some(CcAction::BassKnob(index.min(2), amount)),
        ),
        Route::Consume => (Flow::Consume, None),
        Route::Pass => (Flow::Forward, None),
    };
    CcDecision { flow, action }
}

fn modes() -> Modes {
    let menu = crate::menu::state();
    let screen = crate::paint::screen();
    // SAFETY: `screen` is the component last recorded by the paint hook on
    // this UI callback thread.
    let page_visible = unsafe { crate::page::visible(screen) };
    // SAFETY: the visible Sample screen is the same live component.
    let chop_mode = page_visible && unsafe { crate::page::chop_mode(screen) };
    Modes {
        looper: crate::backend::looper::looper_is_open() != 0,
        spectral: crate::spectral::is_active(),
        menu: menu.open != 0,
        bass_edit: menu.bass_edit != 0,
        busy: menu.busy != 0,
        page_visible,
        trim_screen: crate::ui::trim_screen(),
        chop_mode,
        shift: mpc_sample_sdk::shift(),
        erase: mpc_sample_sdk::erase(),
    }
}

fn track_screen_button(event: ButtonEvent) {
    if !event.pressed {
        return;
    }
    if event.code == buttons::KEY_SCREEN1 {
        crate::ui::set_trim_screen(true);
    } else if event.code == buttons::KEY_B3 && !event.shift {
        // A plain B3 opens the Filter page; SHIFT+B3 is AutoTrim and does not
        // leave the trim page.
        crate::ui::set_trim_screen(false);
    } else if buttons::leaves_trim_screen(event.code) {
        crate::ui::set_trim_screen(false);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ButtonAction, CcAction, EncoderAction, Modes, State, button_flow, button_with_modes,
        decide_button, decide_cc, encoder_with_modes, route_pad, surface_with_modes,
    };
    use crate::buttons::{self, PRESSED};
    use mpc_sample_sdk::{
        ButtonEvent, CcEvent, EncoderEvent, Flow, PadAction, PadEvent, SurfaceEvent,
    };

    fn event_button(code: u32, value: u32, shift: bool) -> ButtonEvent {
        ButtonEvent {
            event: 0,
            bytes: [0; 16],
            code,
            value,
            pressed: value == PRESSED,
            shift,
            erase: false,
            surface: 1,
        }
    }

    fn modes() -> Modes {
        Modes {
            page_visible: true,
            ..Modes::default()
        }
    }

    fn pad(action: PadAction, source: i32) -> PadEvent {
        PadEvent {
            action,
            input: 1,
            source,
            pad: 0,
            properties: 0,
            velocity: if action == PadAction::Press { 0.8 } else { 0.0 },
        }
    }

    #[test]
    fn the_trim_page_survives_sample_and_shift_b3() {
        // The app opens on the trim page; SAMPLE only brings the page forward.
        super::track_screen_button(event_button(buttons::KEY_SCREEN1, PRESSED, false));
        assert!(crate::ui::trim_screen());
        // SAMPLE (0x06) keeps the selected knob page.
        super::track_screen_button(event_button(0x06, PRESSED, false));
        assert!(crate::ui::trim_screen(), "SAMPLE keeps the knob page");
        // SHIFT+B3 is AutoTrim: it must not look like the Filter page key.
        super::track_screen_button(event_button(buttons::KEY_B3, PRESSED, true));
        assert!(crate::ui::trim_screen());
        // A plain B3 opens the Filter page.
        super::track_screen_button(event_button(buttons::KEY_B3, PRESSED, false));
        assert!(!crate::ui::trim_screen());
        super::track_screen_button(event_button(buttons::KEY_SCREEN2, PRESSED, false));
        assert!(!crate::ui::trim_screen());
        super::track_screen_button(event_button(buttons::KEY_SCREEN1, PRESSED, false));
        assert!(crate::ui::trim_screen());
        // A release must not change the page.
        super::track_screen_button(event_button(buttons::KEY_SCREEN2, 0, false));
        assert!(crate::ui::trim_screen());
    }

    #[test]
    fn a_forwarded_pad_keeps_its_release_across_modal_changes_and_sources() {
        let ledger = super::PadLedger::new();
        let last = super::AtomicI32::new(-1);
        let press = route_pad(&ledger, &last, pad(PadAction::Press, 1), modes());
        assert_eq!(press.flow, Flow::Forward);
        assert!(!ledger.finish_press(0, press.token, true));
        let modal = Modes {
            looper: true,
            ..modes()
        };
        assert_eq!(
            route_pad(&ledger, &last, pad(PadAction::Press, 2), modal).flow,
            Flow::Consume
        );
        assert_eq!(
            route_pad(&ledger, &last, pad(PadAction::Release, 2), modes()).flow,
            Flow::Consume
        );
        assert_eq!(
            route_pad(&ledger, &last, pad(PadAction::Release, 1), modal).flow,
            Flow::Forward
        );
    }

    #[test]
    fn a_consumed_octave_press_owns_its_release_after_leaving_16_levels() {
        let mut state = State::new();
        let levels = modes();
        let press = event_button(buttons::KEY_PLUS, PRESSED, false);
        let decision = button_with_modes(&mut state, press, levels, true);
        assert_eq!(decision.action, Some(ButtonAction::BumpOctave(1)));
        assert_eq!(decision.flow, Flow::Consume);
        let release = event_button(buttons::KEY_PLUS, 0, false);
        let decision = button_with_modes(&mut state, release, modes(), false);
        assert_eq!(decision.action, Some(ButtonAction::BumpOctave(0)));
        assert_eq!(decision.flow, Flow::Consume);
        assert!(!state.plus_held);
    }

    #[test]
    fn an_octave_release_stays_owned_if_the_looper_opens_while_held() {
        let mut state = State::new();
        let press = event_button(buttons::KEY_PLUS, PRESSED, false);
        button_with_modes(&mut state, press, modes(), true);
        let looper = Modes {
            looper: true,
            ..modes()
        };
        let release = event_button(buttons::KEY_PLUS, 0, false);
        let decision = button_with_modes(&mut state, release, looper, false);
        assert_eq!(decision.flow, Flow::Consume);
        assert_eq!(decision.action, Some(ButtonAction::BumpOctave(0)));
        assert!(!state.plus_held);
    }

    #[test]
    fn looper_consumes_shift_on_both_edges() {
        let mut state = State::new();
        let looper = Modes {
            looper: true,
            ..modes()
        };
        for value in [PRESSED, 0] {
            let event = event_button(buttons::KEY_SHIFT, value, value == PRESSED);
            assert_eq!(
                button_with_modes(&mut state, event, looper, false).flow,
                Flow::Consume
            );
        }
    }

    #[test]
    fn a_shifted_trim_button_triggers_once_per_press_edge() {
        let mut state = State::new();
        let trim = Modes {
            trim_screen: true,
            ..modes()
        };
        let press = event_button(buttons::KEY_B3, PRESSED, true);
        assert_eq!(
            button_with_modes(&mut state, press, trim, false).action,
            Some(ButtonAction::Autotrim)
        );
        assert_eq!(
            button_with_modes(&mut state, press, trim, false).action,
            None
        );
        let release = event_button(buttons::KEY_B3, 0, true);
        assert_eq!(
            button_with_modes(&mut state, release, trim, false).action,
            None
        );
        assert_eq!(
            button_with_modes(&mut state, press, trim, false).action,
            Some(ButtonAction::Autotrim)
        );
    }

    #[test]
    fn shift_shortcuts_and_sample_page_levels_have_stock_forwarding() {
        let shift = event_button(buttons::KEY_LEVELS, PRESSED, true);
        assert_eq!(
            decide_button(&State::new(), shift, modes(), false),
            ButtonAction::Forward
        );
        let hidden = Modes {
            page_visible: false,
            ..modes()
        };
        let levels = event_button(buttons::KEY_LEVELS, PRESSED, false);
        assert_eq!(
            decide_button(&State::new(), levels, hidden, false),
            ButtonAction::Forward
        );
        assert_eq!(button_flow(ButtonAction::Forward), Flow::Forward);
    }

    #[test]
    fn menu_encoder_decisions_match_surface_and_modal_ownership() {
        let mut state = State::new();
        state.data_target = 10;
        let event = EncoderEvent {
            surface: 10,
            delta: -8,
        };
        let menu = Modes {
            menu: true,
            ..modes()
        };
        assert_eq!(
            encoder_with_modes(&state, event, menu),
            super::EncoderDecision {
                flow: Flow::Consume,
                action: Some(EncoderAction::MenuStep(-1)),
            }
        );
        let synth = Modes {
            menu: true,
            bass_edit: true,
            ..modes()
        };
        assert_eq!(
            encoder_with_modes(&state, event, synth).action,
            Some(EncoderAction::SynthKindStep(-1))
        );
        assert_eq!(
            encoder_with_modes(
                &state,
                EncoderEvent {
                    surface: 11,
                    delta: 1
                },
                menu
            )
            .flow,
            Flow::Forward
        );
    }

    #[test]
    fn surface_pad_preview_is_separate_from_the_sample_pad_path() {
        let modes = Modes {
            menu: true,
            bass_edit: true,
            ..modes()
        };
        let event = SurfaceEvent {
            surface: 1,
            code: 2,
            payload: 0,
            kind: 0x90,
            note: 2,
            velocity: 64,
        };
        let decision = surface_with_modes(event, modes);
        assert_eq!(decision.flow, Flow::Consume);
        assert_eq!(decision.preview.unwrap().pad, 2);
        let pass = surface_with_modes(event, Modes::default());
        assert_eq!(
            pass,
            super::SurfaceDecision {
                flow: Flow::Forward,
                preview: None
            }
        );
    }

    #[test]
    fn cc_decision_preserves_route_ownership_and_master_passthrough() {
        let knob = CcEvent {
            event: 0,
            control: u32::from(crate::routing::CC_KNOB1),
            value: 64,
            low: 0.0,
            range: 127.0,
        };
        let bass = decide_cc(
            knob,
            Modes {
                menu: true,
                bass_edit: true,
                ..modes()
            },
        );
        assert_eq!(bass.flow, Flow::Consume);
        assert_eq!(bass.action, Some(CcAction::BassKnob(0, 64.0 / 127.0)));
        let master = CcEvent {
            control: u32::from(crate::routing::CC_MASTER),
            value: 100,
            ..knob
        };
        let pass = decide_cc(master, modes());
        assert_eq!(pass.flow, Flow::Forward);
        assert_eq!(pass.action, Some(CcAction::Master(100)));
        let modal = decide_cc(
            master,
            Modes {
                menu: true,
                ..modes()
            },
        );
        assert_eq!(modal.flow, Flow::Consume);
    }
}
