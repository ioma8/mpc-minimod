//! The spectral chop's place in the frame.
//!
//! The input hook only raises a request; the work happens from the paint hook,
//! on the UI thread, after the app has drawn.  That is why this is a small state
//! machine rather than a task: one step per frame, three seconds at most.

use crate::menu;
use crate::paint;

/// How long a run may take before it gives up.
const DEADLINE_MS: i64 = 3_000;
/// How long a claim may wait for the page to start it.
const CLAIM_MS: i64 = 5_000;

/// What the chop is doing.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// 1 while a run is in flight, -1 while it is being claimed.
    pub active: i32,
    /// 0 before the sample is read, 1 afterwards.
    pub phase: i32,
    /// How many slices the pad ended up with.
    pub slices: i32,
    /// When the run gives up, in milliseconds.
    pub deadline: i64,
    /// The button event that started it.
    pub event: [u8; 16],
    /// The screen it started on.
    pub screen: usize,
    /// The page state it started with.
    pub page_state: usize,
    /// The sample it is chopping.
    pub sample: usize,
    /// Its frame count.
    pub frames: u32,
    /// Its channel count.
    pub channels: u32,
    /// Its rate.
    pub rate: u32,
}

/// What the glue has to supply.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Press the app's CHOP key.
    pub press_chop: Option<unsafe extern "C" fn()>,
    /// Replace the status line.
    pub set_status: Option<unsafe extern "C" fn(*const u8)>,
    /// Redraw the screen.
    pub repaint: Option<unsafe extern "C" fn()>,
}

/// Claim a run; false when one is already going or a job is busy.
pub fn start(state: &mut State, event: &[u8], busy: bool) -> bool {
    if busy || state.active != 0 {
        return false;
    }
    state.active = 1;
    state.phase = 0;
    state.screen = 0;
    state.slices = 0;
    // The claim expires if the page never paints to start the run.
    state.deadline = crate::clock::now_ms() + CLAIM_MS;
    state.event = [0; 16];
    let length = event.len().min(state.event.len());
    state.event[..length].copy_from_slice(&event[..length]);
    true
}

/// Abandon a run and put the flags back.
pub fn finish(state: &mut State, menu_state: &mut menu::State, hooks: Hooks, error: Option<&str>) {
    let message = match error {
        Some(error) => format!("Spectral: {error}"),
        None => format!("Spectral: {} slices on this pad", state.slices),
    };
    state.phase = 0;
    state.active = 0;
    menu_state.busy = 0;
    if let Some(status) = hooks.set_status {
        let mut text = message.clone().into_bytes();
        text.push(0);
        // SAFETY: the text outlives the call and is NUL-terminated.
        unsafe { status(text.as_ptr()) };
    }
    paint::show_toast(if error.is_some() {
        &message
    } else {
        "Spectral: chopped"
    });
    repaint(hooks);
}

/// Redraw.
fn repaint(hooks: Hooks) {
    if let Some(repaint) = hooks.repaint {
        // SAFETY: the glue's own callback.
        unsafe { repaint() };
    }
}

/// Run one frame of the chop, on the UI thread.
///
/// # Safety
///
/// The hooks must be the glue's callbacks, and `menu_state` the glue's menu.
pub unsafe fn tick(
    state: &mut State,
    menu_state: &mut menu::State,
    base: usize,
    screen: usize,
    hooks: Hooks,
) {
    if state.active != 1 {
        return;
    }
    let now = crate::clock::now_ms();
    if state.phase == 0 {
        start_run(state, menu_state, base, screen, hooks, now);
        return;
    }
    // A claim that never reached the machine must not mute every pad for good.
    if state.phase == 0 && now > state.deadline {
        finish(state, menu_state, hooks, Some("timed out"));
        return;
    }
    let mut slices = 0_u32;
    let mut error: *const u8 = core::ptr::null();
    // SAFETY: the machine is driven from this thread only.
    let step = unsafe {
        crate::spectral_machine::minimod_spectral_step(
            base,
            now,
            core::ptr::from_mut(&mut slices),
            core::ptr::from_mut(&mut error),
        )
    };
    match step {
        3 => {
            state.slices = i32::try_from(slices).unwrap_or(0);
            finish(state, menu_state, hooks, None);
        }
        4 => {
            // SAFETY: the machine returns a NUL-terminated static or null.
            let text = unsafe { error_text(error) };
            finish(state, menu_state, hooks, Some(&text));
        }
        _ => repaint(hooks),
    }
}

/// Read the sample, hand it to the machine and close the menu.
fn start_run(
    state: &mut State,
    menu_state: &mut menu::State,
    base: usize,
    screen: usize,
    hooks: Hooks,
    now: i64,
) {
    let mut found_screen: usize = 0;
    let mut page_state: usize = 0;
    let mut sample: usize = 0;
    let mut frames: u32 = 0;
    let mut channels: u32 = 0;
    let mut rate: u32 = 0;
    // SAFETY: the page objects are read, not written.
    let found = unsafe {
        crate::selection::minimod_sample_now(
            base,
            screen,
            core::ptr::from_mut(&mut found_screen),
            core::ptr::from_mut(&mut page_state),
            core::ptr::from_mut(&mut sample),
            core::ptr::from_mut(&mut frames),
            core::ptr::from_mut(&mut channels),
            core::ptr::from_mut(&mut rate),
        )
    };
    if found == 0 {
        finish(state, menu_state, hooks, Some("no sample on this pad"));
        return;
    }
    // The loaded audio first, so a recording that was never saved still works;
    // a streamed sample falls back to its WAV.
    let channels16 = u16::try_from(channels).unwrap_or(0);
    let samples = crate::sample::memory(sample)
        .filter(|memory| {
            memory.frames == frames && u32::from(memory.channels) == channels && memory.rate == rate
        })
        .and_then(|memory| memory.read())
        .or_else(|| {
            let path = crate::paths::for_sample(sample, frames, channels16, rate)?;
            crate::wav::read_f32(&path, frames, channels16, rate)
        });
    let Some(samples) = samples else {
        finish(state, menu_state, hooks, Some("cannot read sample audio"));
        return;
    };
    let mut error: *const u8 = core::ptr::null();
    // SAFETY: the samples live for the call and the error is writable.
    let begun = unsafe {
        crate::spectral_machine::minimod_spectral_begin(
            base,
            now,
            found_screen,
            page_state,
            sample,
            frames,
            channels,
            rate,
            samples.as_ptr(),
            hooks.press_chop,
            core::ptr::from_mut(&mut error),
        )
    };
    if begun != 0 {
        // SAFETY: the machine returns a NUL-terminated static or null.
        let text = unsafe { error_text(error) };
        finish(state, menu_state, hooks, Some(&text));
        return;
    }
    menu_state.open = 0;
    menu_state.busy = 1;
    state.phase = 1;
    state.screen = found_screen;
    state.page_state = page_state;
    state.sample = sample;
    state.frames = frames;
    state.channels = channels;
    state.rate = rate;
    state.deadline = now + DEADLINE_MS;
    repaint(hooks);
}

/// The text the machine reports, as an owned string.
///
/// # Safety
///
/// `error` must be null or a NUL-terminated string.
unsafe fn error_text(error: *const u8) -> String {
    if error.is_null() {
        return String::from("unknown");
    }
    // SAFETY: the caller's contract.
    unsafe { core::ffi::CStr::from_ptr(error.cast()) }
        .to_string_lossy()
        .into_owned()
}

/// Claim a run, for the C glue.
///
/// # Safety
///
/// `state` must be live and `event` readable for `length` bytes.
pub unsafe extern "C" fn minimod_spectral_start(
    state: *mut State,
    event: *const u8,
    length: i32,
    busy: i32,
) -> i32 {
    if state.is_null() || event.is_null() || length <= 0 {
        return 0;
    }
    let length = usize::try_from(length).unwrap_or(0);
    // SAFETY: the caller's contract.
    let bytes = unsafe { core::slice::from_raw_parts(event, length) };
    // SAFETY: the caller's contract.
    let state = unsafe { &mut *state };
    i32::from(start(state, bytes, busy != 0))
}

/// Run one frame, for the C glue.
///
/// # Safety
///
/// `state` and `menu_state` must be live and `hooks` readable.
pub unsafe extern "C" fn minimod_spectral_tick(
    state: *mut State,
    menu_state: *mut core::ffi::c_void,
    base: usize,
    screen: usize,
    hooks: *const Hooks,
) {
    if state.is_null() || menu_state.is_null() || hooks.is_null() {
        return;
    }
    // SAFETY: the caller's contract.
    let (state, menu_state, hooks) =
        unsafe { (&mut *state, &mut *menu_state.cast::<menu::State>(), *hooks) };
    // SAFETY: the hooks are the glue's callbacks.
    unsafe { tick(state, menu_state, base, screen, hooks) };
}

/// End a run whose page went away.  It is called from the input hooks, which
/// run whether or not the Sample page paints.
///
/// # Safety
///
/// Reads the app's page objects; the state is the crate's own.
pub unsafe fn cancel_if_page_gone() {
    // SAFETY: the statics are the crate's own, written by the UI thread.
    let state = unsafe { &mut *core::ptr::addr_of_mut!(G_SPECTRAL) };
    if state.active != 1 {
        return;
    }
    let screen = paint::screen();
    let visible = crate::page::minimod_page_visible(crate::firmware::base(), screen) != 0;
    if visible && (state.screen == 0 || screen == state.screen) {
        return;
    }
    let hooks = Hooks {
        press_chop: None,
        set_status: Some(minimod_status_c),
        repaint: Some(crate::ui::minimod_repaint_screen),
    };
    finish(state, menu::state_mut(), hooks, Some("page changed"));
}

/// The run in flight.  It keeps its name because the glue reads it.
pub static mut G_SPECTRAL: State = State {
    active: 0,
    phase: 0,
    slices: 0,
    deadline: 0,
    event: [0; 16],
    screen: 0,
    page_state: 0,
    sample: 0,
    frames: 0,
    channels: 0,
    rate: 0,
};

/// Press the app's own CHOP key on the event that started the run.
///
/// # Safety
///
/// The app's own dispatch is used.
pub unsafe extern "C" fn minimod_spectral_key_press() {
    // SAFETY: the event is this module's and the dispatch the app's.
    unsafe {
        let Some(original) = crate::ui::original_buttons() else {
            return;
        };
        let mut event = (*core::ptr::addr_of!(G_SPECTRAL)).event;
        // Only replay a button that was really captured; its context is the
        // app's own and a blank one would be dereferenced.
        if event[..8] == [0; 8] {
            return;
        }
        event[8] = 0x4b;
        event[9] = 0x7f;
        original(event.as_ptr() as usize);
        event[9] = 0x00;
        original(event.as_ptr() as usize);
    }
}

/// One frame of the chop's state machine, driven from the paint.
///
/// # Safety
///
/// The app's own calls are used.
pub unsafe extern "C" fn minimod_spectral_frame(base: usize, screen: usize) {
    let hooks = Hooks {
        press_chop: Some(minimod_spectral_key_press),
        set_status: Some(minimod_status_c),
        repaint: Some(crate::ui::minimod_repaint_screen),
    };
    // SAFETY: the state and the menu are the crate's own, and the hooks are above.
    unsafe {
        minimod_spectral_tick(
            core::ptr::addr_of_mut!(G_SPECTRAL),
            core::ptr::addr_of_mut!(menu::g_menu).cast::<core::ffi::c_void>(),
            base,
            screen,
            core::ptr::from_ref(&hooks),
        );
    }
}

/// The status setter, in the shape the hooks want.
unsafe extern "C" fn minimod_status_c(text: *const u8) {
    if text.is_null() {
        return;
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let text = unsafe { core::ffi::CStr::from_ptr(text.cast()) };
    crate::status::set_status(&text.to_string_lossy());
}

/// Start a chop from the crate's own record and the event that started it.
///
/// # Safety
///
/// Reads the crate's own state.
pub unsafe extern "C" fn minimod_spectral_run_start(busy: i32) -> i32 {
    let event = crate::ui::event_copy();
    // SAFETY: the record is this module's.
    unsafe {
        minimod_spectral_start(
            core::ptr::addr_of_mut!(G_SPECTRAL),
            event.as_ptr(),
            i32::try_from(event.len()).unwrap_or(0),
            busy,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{State, start};

    #[test]
    fn a_run_cannot_start_twice() {
        let mut state = State::default();
        assert!(start(&mut state, &[1, 2, 3], false));
        assert!(!start(&mut state, &[4], false), "already running");
        let mut idle = State::default();
        assert!(!start(&mut idle, &[1], true), "job busy");
    }

    #[test]
    fn the_event_is_copied_in() {
        let mut state = State::default();
        assert!(start(&mut state, &[9, 8, 7], false));
        assert_eq!(&state.event[..3], &[9, 8, 7]);
        assert_eq!(state.event.len(), 16);
    }
}
