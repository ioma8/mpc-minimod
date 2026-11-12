//! Copy the Live Looper's closed loops into new sampler pads.

use core::ffi::CStr;

use mpc_sample_sdk::platform::extract;

use crate::backend::looper::{
    looper_close, looper_copy_loop, looper_loop_len, looper_notice, looper_stop_all,
};

/// One name per looper pad, used as the new sample's name.
const NAMES: [&CStr; 16] = [
    c"Loop 1", c"Loop 2", c"Loop 3", c"Loop 4", c"Loop 5", c"Loop 6", c"Loop 7", c"Loop 8",
    c"Loop 9", c"Loop 10", c"Loop 11", c"Loop 12", c"Loop 13", c"Loop 14", c"Loop 15", c"Loop 16",
];

/// The first `count` empty pads on the live Sample page, or `None`.
fn free_pads(count: usize) -> Option<Vec<i32>> {
    let surface = crate::control::surface();
    if surface == 0 {
        return None;
    }
    let mut found = Vec::with_capacity(count);
    for pad in 0..128i32 {
        if found.len() == count {
            break;
        }
        if crate::pads::sample_for_pad(surface, pad).is_none() {
            found.push(pad);
        }
    }
    (found.len() == count).then_some(found)
}

/// Stop the loops, write every closed loop to an empty pad and report.
///
/// Returns `true` when the caller may close the looper: every loop was copied,
/// or there was nothing to copy. A partial copy or a full ring keeps the view
/// open with a notice.
#[must_use]
pub fn copy_all() -> bool {
    looper_stop_all();
    let mut loops = Vec::new();
    for pad in 0..NAMES.len() {
        let len = looper_loop_len(pad);
        if len == 0 {
            continue;
        }
        let Ok(frames) = usize::try_from(len) else {
            continue;
        };
        let mut audio = vec![0.0_f32; frames];
        if looper_copy_loop(pad, &mut audio) != len {
            looper_notice("COPY: read failed");
            return false;
        }
        loops.push((pad, audio));
    }
    if loops.is_empty() {
        looper_close();
        crate::paint::show_toast("Looper: no loops");
        return true;
    }
    let Some(destinations) = free_pads(loops.len()) else {
        let message = format!("COPY: need {} empty pads", loops.len());
        looper_notice(&message);
        return false;
    };
    // Extract must run on the native Sample page, not under the fullscreen
    // looper overlay. The loops are already copied into `loops`, so closing
    // first keeps the engine's buffers untouched.
    looper_close();
    let screen = crate::paint::screen();
    let Some(state) = crate::selection::page_state(screen) else {
        crate::paint::show_toast("Looper: no Sample page");
        return false;
    };
    for ((pad, audio), _destination) in loops.iter().zip(destinations) {
        let Ok(frames) = u32::try_from(audio.len()) else {
            crate::paint::show_toast("Looper: copy failed");
            return false;
        };
        // Use the same buffered Extract path as HPSS rather than the seeded
        // `generated` path: it lets the app assign the next free pad itself and
        // remains stable across several writes in one command.
        let request = extract::Request {
            screen,
            state,
            name: NAMES[*pad],
            buffer: Some(audio),
            source_frames: frames,
            channels: 1,
            start: 0,
            end: frames,
            destination: None,
        };
        // SAFETY: `screen` and `state` are the live Sample page on this UI
        // thread and the audio stays alive through the synchronous call.
        if unsafe { extract::run(&request) }.is_err() {
            crate::paint::show_toast("Looper: copy failed");
            return false;
        }
    }
    crate::paint::show_toast(&format!("Looper: copied {} loops", loops.len()));
    true
}
