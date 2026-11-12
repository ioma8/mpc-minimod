//! The effects menu and the bass control page.
//!
//! The menu is a modal screen the mod draws itself: SHIFT + the encoder opens
//! it, the encoder picks a row, and a press runs it.  The state it keeps is the
//! open flag, the selected row and whether the bass control page is showing.

use crate::actions::Action;
use crate::backend::bass::Kind;
use crate::paint;
use mpc_sample_sdk::export_static;
use mpc_sample_sdk::state::Global;

/// The rows of the effects menu.
/// The split.
pub const ROW_HPSS: i32 = 0;
/// The spectral chop.
pub const ROW_SPECTRAL: i32 = 1;
/// Reverb.
pub const ROW_REVERB: i32 = 2;
/// Vinyl.
pub const ROW_VINYL: i32 = 3;
/// Lo-Fi.
pub const ROW_LOFI: i32 = 4;
/// The bass synth.
pub const ROW_BASS: i32 = 5;
/// The live looper.
pub const ROW_LOOPER: i32 = 6;

/// The menu's state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// Whether the menu is showing.
    pub open: i32,
    /// Whether a job is running.
    pub busy: i32,
    /// Whether the bass control page is showing.
    pub bass_edit: i32,
    /// The selected row.
    pub selection: i32,
}

/// The feature callbacks the menu uses.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Redraw the screen.
    pub repaint: Option<fn()>,
    /// Render the split.
    pub start_split: Option<fn(&mut State)>,
    /// Render one effect.
    pub start_effect: Option<fn(Action, &mut State)>,
    /// Start the spectral chop, with the event that asked for it.
    pub start_spectral: Option<fn(&mut State, &[u8; 16])>,
    /// Start the bass synth with its source backup.
    pub start_bass: Option<fn(&mut State)>,
    /// Show the bass control page.
    pub start_bass_page: Option<fn(&mut State)>,
    /// Open the live looper.
    pub start_looper: Option<fn() -> i32>,
}

/// Close the menu unless a job is running.
pub fn close(state: &mut State, hooks: Hooks) -> bool {
    if state.busy != 0 {
        return false;
    }
    if state.bass_edit != 0 {
        crate::synth_lights::hide();
    }
    state.open = 0;
    state.bass_edit = 0;
    repaint(hooks);
    true
}

/// Run the selected row.
pub fn activate(state: &mut State, event: &[u8; 16], hooks: Hooks) {
    if state.busy != 0 {
        return;
    }
    if state.bass_edit != 0 {
        if let Some(start) = hooks.start_bass {
            start(state);
        }
        return;
    }
    match state.selection {
        ROW_HPSS => {
            if let Some(start) = hooks.start_split {
                start(state);
            }
        }
        ROW_SPECTRAL => {
            if let Some(start) = hooks.start_spectral {
                start(state, event);
            }
        }
        ROW_REVERB => {
            if let Some(start) = hooks.start_effect {
                start(Action::Reverb, state);
            }
        }
        ROW_VINYL => {
            if let Some(start) = hooks.start_effect {
                start(Action::Vinyl, state);
            }
        }
        ROW_LOFI => {
            if let Some(start) = hooks.start_effect {
                start(Action::LoFi, state);
            }
        }
        ROW_BASS => {
            if let Some(page) = hooks.start_bass_page {
                page(state);
            }
            repaint(hooks);
        }
        ROW_LOOPER => {
            if let Some(activate) = hooks.start_looper {
                let opened = activate();
                if opened != 0 {
                    if state.bass_edit != 0 {
                        crate::synth_lights::hide();
                    }
                    state.open = 0;
                    state.bass_edit = 0;
                }
            }
            repaint(hooks);
        }
        _ => {
            close(state, hooks);
        }
    }
}

/// Redraw.
fn repaint(hooks: Hooks) {
    if let Some(repaint) = hooks.repaint {
        repaint();
    }
}

/// The menu's colours.
const PANEL: u32 = 0xff10_1014;
const ACCENT: u32 = 0xffff_b43c;
const WHITE: u32 = 0xffff_ffff;
const MUTED: u32 = 0xffb8_b8be;
const DIM: u32 = 0xff48_484f;
const SLOT: u32 = 0xff36_363d;
const ROW: u32 = 0xff3a_2a10;

/// The rows, in the order the encoder walks them.
const TITLES: [&str; MENU_ROWS] = [
    "HPSS",
    "Spectral Chop",
    "Add Reverb",
    "Vinyl",
    "Lo-Fi (Decimator)",
    "Synth",
    "Live Looper",
    "Close",
];

/// Draw the menu.
///
/// `status` is the caller's status line (empty when there is nothing to say) and
/// `knobs` are the bass synth's three values.
pub fn draw(graphics: usize, state: &State, busy: bool, status: &str, knobs: [f32; 3]) {
    let (x, y, w, h) = (14, 8, 292, 224);
    paint::colour(graphics, PANEL);
    paint::rect(graphics, x, y, w, h);
    paint::colour(graphics, ACCENT);
    paint::rect(graphics, x, y, w, 2);
    paint::rect(graphics, x, y + h - 2, w, 2);
    if state.bass_edit == 0 {
        paint::colour(graphics, WHITE);
        paint::text(graphics, "SAMPLE OPTIONS", x + 10, y + 6, w - 20, 22, 18.0);
    }
    if busy {
        paint::text(graphics, "Processing...", x + 10, y + 70, w - 20, 24, 16.0);
        if !status.is_empty() {
            paint::text(graphics, status, x + 10, y + 98, w - 20, 20, 12.0);
        }
        return;
    }
    if state.bass_edit != 0 {
        draw_knobs(graphics, (x, y, w), status, knobs);
        return;
    }
    for (index, title) in TITLES.iter().enumerate() {
        let row = y + 26 + i32::try_from(index).unwrap_or(0) * 20;
        if state.selection == i32::try_from(index).unwrap_or(-1) {
            paint::colour(graphics, ROW);
            paint::rect(graphics, x + 6, row, w - 12, 19);
            paint::colour(graphics, WHITE);
        }
        paint::text(graphics, title, x + 12, row + 1, w - 24, 17, 12.0);
    }
    if !status.is_empty() {
        paint::text(graphics, status, x + 10, y + 200, w - 20, 14, 10.0);
    }
    paint::colour(graphics, 0xffb0_b0b0);
    paint::text(
        graphics,
        "TURN: select    DATA: confirm",
        x + 10,
        y + h - 17,
        w - 20,
        14,
        10.0,
    );
}

/// The bass control page.
fn draw_knobs(graphics: usize, area: (i32, i32, i32), status: &str, knobs: [f32; 3]) {
    let (x, y, w) = area;
    let kind = bass_kind();
    let labels = kind.knobs();
    paint::colour(graphics, ACCENT);
    paint::text_fmt(
        graphics,
        format_args!("SYNTH  {}", kind.name()),
        x + 12,
        y + 6,
        w - 24,
        22,
        18.0,
    );
    paint::colour(graphics, MUTED);
    paint::text(
        graphics,
        "PADS C2-D#3  /  CHROMATIC",
        x + 12,
        y + 36,
        w - 24,
        16,
        11.0,
    );
    let note = crate::backend::bass::note_name(crate::backend::bass::last_note_semitones());
    paint::colour(graphics, ACCENT);
    paint::text_fmt(
        graphics,
        format_args!("NOTE {note}"),
        x + 12,
        y + 52,
        w - 24,
        16,
        11.0,
    );
    let columns = [x + 16, x + 108, x + 200];
    for (index, column) in columns.iter().enumerate() {
        paint::colour(graphics, ACCENT);
        paint::text_fmt(
            graphics,
            format_args!("K{}", index + 1),
            *column,
            y + 72,
            76,
            16,
            11.0,
        );
        paint::colour(graphics, WHITE);
        paint::text(graphics, labels[index], *column, y + 90, 76, 18, 12.0);
        paint::colour(graphics, SLOT);
        paint::rect(graphics, *column, y + 116, 76, 4);
        paint::colour(graphics, ACCENT);
        let filled = (76.0 * knobs[index].clamp(0.0, 1.0) + 0.5) as i32;
        paint::rect(graphics, *column, y + 116, filled, 4);
        paint::colour(graphics, WHITE);
        paint::text_fmt(
            graphics,
            format_args!("{:.2}", knobs[index]),
            *column,
            y + 128,
            76,
            20,
            14.0,
        );
    }
    paint::colour(graphics, DIM);
    paint::rect(graphics, x + 12, y + 164, w - 24, 1);
    if !status.is_empty() {
        paint::colour(graphics, 0xffe8_e8ec);
        paint::text(graphics, status, x + 12, y + 168, w - 24, 14, 10.0);
    }
    paint::colour(graphics, ACCENT);
    paint::text(
        graphics,
        "PADS: PLAY   DATA: SAVE   SHIFT+DATA: BACK",
        x + 10,
        y + 192,
        w - 20,
        16,
        10.0,
    );
}

/// The number of rows.
pub const MENU_ROWS: usize = 8;

/// Draw the menu.
pub fn draw_menu(
    graphics: usize,
    selection: i32,
    bass_edit: i32,
    busy: i32,
    status: &str,
    knobs: [f32; 3],
) {
    let state = State {
        open: 1,
        busy,
        bass_edit,
        selection,
    };
    draw(graphics, &state, busy != 0, status, knobs);
}

export_static!(#[doc = "The menu's state, read by the device tests."]
g_menu: Global<State> = Global::new(State {
    open: 0,
    busy: 0,
    bass_edit: 0,
    selection: 0,
}));

/// A copy of the menu's state, for the crate.
#[must_use]
pub fn state() -> State {
    // SAFETY: `g_menu` is owned by the app's UI thread.
    unsafe { g_menu.with(|menu| *menu) }
}

/// Run `f` with the menu's state, to change it.
pub fn with_state_mut<R>(f: impl FnOnce(&mut State) -> R) -> R {
    // SAFETY: `g_menu` is owned by the app's UI thread and the closure must not
    // take another borrow of it.
    unsafe { g_menu.with_mut(f) }
}

export_static!(#[doc = "The bass page's three knobs, read by the device tests."]
g_bass_knob: Global<[f32; 3]> = Global::new(Kind::Bass.defaults()));

export_static!(#[doc = "The selected synth character, read by the device tests."]
g_bass_kind: Global<i32> = Global::new(0));

/// The selected synth character.
#[must_use]
pub fn bass_kind() -> Kind {
    // SAFETY: `g_bass_kind` is owned by the app's UI thread.
    unsafe { g_bass_kind.with(|kind| Kind::from_index(*kind)) }
}

/// Select a synth character and reset its three knobs to that character's
/// defaults.
pub fn set_bass_kind(kind: Kind) {
    // SAFETY: both values are owned by the app's UI thread; the borrows are
    // sequential, not nested.
    unsafe {
        g_bass_kind.with_mut(|slot| *slot = i32::try_from(kind.index()).unwrap_or(0));
        g_bass_knob.with_mut(|knobs| *knobs = kind.defaults());
    }
}

/// Move the synth selection by `delta` encoder steps.
#[must_use]
pub fn step_bass_kind(delta: i32) -> Kind {
    let kind = bass_kind().step(delta);
    set_bass_kind(kind);
    kind
}

/// Keep the bass preview on while its page is open.
pub fn follow_preview() {
    let menu = state();
    if menu.open != 0
        && menu.bass_edit != 0
        && menu.busy == 0
        && crate::backend::bass::available()
        && !crate::backend::bass::preview_active()
    {
        crate::backend::bass::preview_set_active(true);
    }
}

/// The bass control page's three knobs.
#[must_use]
pub fn bass_knobs() -> [f32; 3] {
    // SAFETY: `g_bass_knob` is owned by the app's UI thread.
    unsafe { g_bass_knob.with(|knobs| *knobs) }
}

/// Store the bass control page's three knobs.
pub fn set_bass_knobs(knobs: [f32; 3]) {
    // SAFETY: `g_bass_knob` is owned by the app's UI thread.
    unsafe { g_bass_knob.with_mut(|slot| *slot = knobs) };
}

/// Show the bass control page and start its preview.
pub fn bass_page(state: &mut State) {
    let knobs = bass_knobs();
    crate::ui::reset_synth_octave();
    state.bass_edit = 1;
    crate::synth_lights::show();
    crate::backend::bass::preview_set_kind(bass_kind());
    crate::backend::bass::preview_set_params(knobs[0], knobs[1], knobs[2]);
    crate::backend::bass::preview_set_active(true);
    let available = crate::backend::bass::available();
    crate::status::set_status(if available {
        ""
    } else {
        "Live preview unavailable"
    });
}

/// Open the effects menu, if the page is showing and nothing is running.
pub fn open_menu() {
    if !crate::ui::screen_visible() || state().busy != 0 {
        return;
    }
    let was_bass = state().bass_edit != 0;
    with_state_mut(|menu| {
        menu.open = 1;
        menu.selection = 0;
        menu.bass_edit = 0;
    });
    if was_bass {
        crate::synth_lights::hide();
    }
    crate::backend::bass::preview_set_active(false);
    crate::status::set_status("");
    crate::ui::minimod_repaint_screen();
}

/// Close the menu, unless a job is running.
pub fn close_menu() {
    if state().busy != 0 {
        return;
    }
    let was_bass = state().bass_edit != 0;
    with_state_mut(|menu| {
        menu.open = 0;
        menu.bass_edit = 0;
    });
    if was_bass {
        crate::synth_lights::hide();
    }
    crate::backend::bass::preview_set_active(false);
    crate::ui::minimod_repaint_screen();
}

/// Rename the soft key, which the app draws itself.
pub fn filter_text() {
    if paint::screen() != 0 {
        crate::ui::minimod_repaint_screen();
    }
}

/// Whether the menu is open.
#[must_use]
pub fn is_open() -> bool {
    state().open != 0
}

/// Whether a job is running.
#[must_use]
pub fn is_busy() -> bool {
    state().busy != 0
}

#[cfg(test)]
mod tests {
    use super::{Hooks, State, close};

    fn hooks() -> Hooks {
        Hooks {
            repaint: None,
            start_split: None,
            start_effect: None,
            start_spectral: None,
            start_bass: None,
            start_bass_page: None,
            start_looper: None,
        }
    }

    #[test]
    fn a_running_job_keeps_the_menu_open() {
        let mut state = State {
            open: 1,
            busy: 1,
            ..State::default()
        };
        assert!(!close(&mut state, hooks()));
        assert_eq!(state.open, 1);
        state.busy = 0;
        assert!(close(&mut state, hooks()));
        assert_eq!(state.open, 0);
    }
}
