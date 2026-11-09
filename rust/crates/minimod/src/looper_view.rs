//! The looper's fullscreen view: a 4x4 pad grid, a sync pulse and a status line.
//!
//! Everything is read from one snapshot of the engine, so the paint never races
//! the audio thread: if the snapshot fails there is nothing to draw.

use crate::backend::looper::{MAX_FRAMES, PADS, PadView, RATE, State, View};

use crate::paint;

/// The screen the view is drawn on.
const WIDTH: i32 = 320;
/// Its height.
const HEIGHT: i32 = 240;

const BACKDROP: u32 = 0xff0c_0c10;
const BAR: u32 = 0xff15_151a;
const ACCENT: u32 = 0xffff_b43c;
const WHITE: u32 = 0xffff_ffff;
const MUTED: u32 = 0xff8a_8a92;
const METER_BACK: u32 = 0xff2a_2a32;
const METER_OK: u32 = 0xff54_d06a;
const METER_HOT: u32 = 0xffff_4d4d;

/// Where a pad cell goes.
#[derive(Clone, Copy)]
struct Cell {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// One pad's colours and label.
fn palette(state: State) -> (u32, u32, u32, &'static str) {
    match state {
        State::Rec => (0xff3a_1010, 0xffff_4d4d, 0xffff_b0b0, "REC"),
        State::Play => (0xff10_301a, METER_OK, 0xffc8_f5d0, "PLAY"),
        State::Dub => (0xff35_2808, ACCENT, 0xffff_e0a8, "DUB"),
        State::Stop => (0xff20_2028, 0xff6a_6a74, 0xffc0_c0c8, "STOP"),
        _ => (0xff1a_1a20, 0xff33_333c, MUTED, "--"),
    }
}

/// Draw one pad cell.
fn cell(graphics: usize, area: Cell, pad: &PadView, index: usize, recording: bool) {
    let Cell {
        x,
        y,
        width,
        height,
    } = area;
    let state = State::from_raw(i32::from(pad.state));
    let (background, border, text, label) = palette(state);
    paint::colour(graphics, background);
    paint::rect(graphics, x, y, width, height);
    paint::colour(graphics, border);
    paint::rect(graphics, x, y, width, 1);
    paint::rect(graphics, x, y + height - 1, width, 1);
    paint::rect(graphics, x, y, 1, height);
    paint::rect(graphics, x + width - 1, y, 1, height);
    if recording {
        paint::colour(graphics, WHITE);
        paint::rect(graphics, x + 1, y + 1, width - 2, 1);
    }
    paint::colour(graphics, text);
    paint::text(
        graphics,
        &format!("{}", index + 1),
        x + 4,
        y + 2,
        18,
        12,
        10.0,
    );
    paint::text(graphics, label, x + 20, y + 2, width - 22, 12, 9.0);

    // A take still capturing its tail is a recording as far as the user sees.
    let recording_state = matches!(state, State::Rec | State::Drain);
    let total = if recording_state {
        u32::try_from(MAX_FRAMES).unwrap_or(u32::MAX)
    } else {
        pad.len_frames
    };
    let done = if recording_state {
        pad.rec_frames
    } else {
        pad.pos
    };
    paint::colour(graphics, METER_BACK);
    paint::rect(graphics, x + 4, y + height - 9, width - 8, 4);
    if total > 0 {
        let span = u64::from(u32::try_from(width - 8).unwrap_or(1));
        let filled = (u64::from(done) * span / u64::from(total)).min(span);
        let filled = i32::try_from(filled).unwrap_or(0);
        paint::colour(
            graphics,
            if recording_state {
                0xffff_6a6a
            } else {
                METER_OK
            },
        );
        paint::rect(graphics, x + 4, y + height - 9, filled, 4);
    }
    if !recording_state && pad.len_frames > 0 {
        let seconds = f64::from(pad.len_frames) / f64::from(RATE);
        paint::colour(graphics, 0xffb8_b8be);
        paint::text(
            graphics,
            &format!("{seconds:.1}s"),
            x + 4,
            y + 15,
            width - 8,
            11,
            8.0,
        );
    }
}

/// The backdrop and the title bar.
fn frame(graphics: usize) {
    paint::colour(graphics, BACKDROP);
    paint::rect(graphics, 0, 0, WIDTH, HEIGHT);
    paint::colour(graphics, BAR);
    paint::rect(graphics, 0, 0, WIDTH, 24);
    paint::colour(graphics, ACCENT);
    paint::rect(graphics, 0, 24, WIDTH, 2);
    paint::colour(graphics, WHITE);
    paint::text(graphics, "LIVE LOOPER", 8, 2, 180, 20, 15.0);
}

/// The sync pulse, or "FREE" when it is off.
fn pulse(graphics: usize, view: &View) {
    if view.sync == 0 || view.sync_frames == 0 {
        paint::colour(graphics, MUTED);
        paint::text(graphics, "FREE", 248, 4, 60, 16, 10.0);
        return;
    }
    let seconds = f64::from(view.sync_frames) / f64::from(RATE);
    let line = if view.click != 0 {
        format!("SYNC {seconds:.2}s +CLK")
    } else {
        format!("SYNC {seconds:.2}s")
    };
    paint::colour(graphics, ACCENT);
    paint::text(graphics, &line, 200, 4, 100, 16, 10.0);
    let beat = view.pulse_phase != u32::MAX && view.pulse_phase < view.sync_frames / 6;
    paint::colour(graphics, if beat { WHITE } else { ACCENT });
    paint::rect(graphics, 305, 8, 8, 8);
    let span = u64::from(view.sync_frames.max(1));
    let travelled = u64::from(view.pulse_phase.min(view.sync_frames)) * u64::from(WIDTH as u32);
    let filled = i32::try_from(travelled / span).unwrap_or(WIDTH);
    paint::colour(graphics, 0xff3a_2a10);
    paint::rect(graphics, 0, 236, WIDTH, 3);
    paint::colour(graphics, ACCENT);
    paint::rect(graphics, 0, 236, filled.clamp(0, WIDTH), 3);
}

/// The input meter.
fn meter(graphics: usize, view: &View) {
    paint::colour(graphics, MUTED);
    paint::text(graphics, "IN", 4, 27, 16, 10, 7.0);
    paint::colour(graphics, METER_BACK);
    paint::rect(graphics, 22, 30, 290, 4);
    paint::colour(
        graphics,
        if view.input > 0.9 {
            METER_HOT
        } else {
            METER_OK
        },
    );
    let level = crate::backend::looper::level(view.input);
    let level = i32::try_from(level).unwrap_or(0);
    if level > 0 {
        paint::rect(graphics, 22, 30, level, 4);
    }
}

/// The 4x4 pad grid position, with pad 1 at the physical bottom-left.
fn grid_position(index: usize) -> (i32, i32) {
    let column = i32::try_from(index % 4).unwrap_or(0);
    // Pads run left-to-right and bottom-to-top: A1 is the bottom-left cell and
    // A16 is the top-right one, matching the hardware below the screen.
    let row = i32::try_from(PADS / 4 - 1 - index / 4).unwrap_or(0);
    (8 + column * 78, 40 + row * 41)
}

/// The 4x4 pad grid.
fn grid(graphics: usize, view: &View) {
    for index in 0..PADS {
        let (x, y) = grid_position(index);
        let recording = view.recording_pad == i32::try_from(index).unwrap_or(-1);
        cell(
            graphics,
            Cell {
                x,
                y,
                width: 75,
                height: 38,
            },
            &view.pads[index],
            index,
            recording,
        );
    }
}

/// The status line and the key hints.
fn footer(graphics: usize, view: &View) {
    let message = message_of(view);
    if !message.is_empty() {
        paint::colour(graphics, 0xff20_1408);
        paint::rect(graphics, 8, 202, 304, 12);
        paint::colour(graphics, ACCENT);
        paint::text(graphics, &message, 12, 202, 296, 12, 10.0);
    } else if view.lat_frames > 0 {
        let source = match view.lat_src {
            b'B' => "BUF",
            b'F' => "FIX",
            _ => "EST",
        };
        let millis = u64::from(view.lat_frames) * 1000 / u64::from(RATE);
        paint::colour(graphics, 0xff5a_5a64);
        paint::rect(graphics, 8, 202, 304, 12);
        paint::colour(graphics, 0xffb8_b8be);
        paint::text(
            graphics,
            &format!("{source} {millis}ms"),
            12,
            202,
            296,
            12,
            9.0,
        );
    }
    paint::colour(graphics, 0xffb8_b8be);
    paint::text(
        graphics,
        "PAD REC/PLAY/STOP  SHIFT OVERDUB  ERASE CLEAR",
        8,
        214,
        304,
        9,
        7.0,
    );
    paint::text(
        graphics,
        "STOP ALL  LOOP SYNC  SHIFT+LOOP CLICK  SHIFT+DATA EXIT",
        8,
        224,
        304,
        9,
        7.0,
    );
}

/// Draw the whole view.
pub fn draw(graphics: usize) {
    let Some(view) = snapshot() else {
        return;
    };
    frame(graphics);
    pulse(graphics, &view);
    meter(graphics, &view);
    grid(graphics, &view);
    footer(graphics, &view);
}

/// Take a snapshot of the engine, or nothing when it is not running.
fn snapshot() -> Option<View> {
    let mut view = View::default();
    // the view is ours and only this thread reads it.
    let ok = crate::backend::looper::looper_snapshot(&mut view);
    (ok != 0).then_some(view)
}

/// The engine's status line, as text.
fn message_of(view: &View) -> String {
    let end = view
        .message
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(view.message.len());
    String::from_utf8_lossy(&view.message[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::message_of;
    use crate::backend::looper::{MESSAGE_LEN, PadView, View};

    fn blank() -> View {
        View {
            message: [0; MESSAGE_LEN],
            pads: [PadView::default(); 16],
            ..View::default()
        }
    }

    #[test]
    fn a_status_line_is_read_to_its_terminator() {
        let mut view = blank();
        view.message[..4].copy_from_slice(b"LOOP");
        assert_eq!(message_of(&view), "LOOP");
        assert_eq!(message_of(&blank()), "");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::grid_position;

    #[test]
    fn pad_one_is_bottom_left_and_pad_sixteen_is_top_right() {
        assert_eq!(grid_position(0), (8, 40 + 3 * 41));
        assert_eq!(grid_position(15), (8 + 3 * 78, 40));
        assert_eq!(grid_position(3), (8 + 3 * 78, 40 + 3 * 41));
        assert_eq!(grid_position(12), (8, 40));
    }
}
