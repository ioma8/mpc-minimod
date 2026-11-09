//! The app's input hooks: pad presses, their releases, control-surface events
//! and the encoder.
//!
//! These run on the app's own threads, so the state they keep is behind one
//! struct the glue owns.  Each hook decides whether the mod consumes the event
//! and otherwise forwards it to the displaced function.

/// Pads the app can address.
pub const PADS: usize = 128;
/// Pads the looper owns.
pub const LOOPER_PADS: i32 = 16;
/// Pads the bass preview listens to.
pub const PREVIEW_PADS: i32 = 16;
/// Entries on the effects menu.
pub const MENU_ITEMS: i32 = 8;

/// One remembered pad press, so the page can be re-selected later.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PadRecord {
    /// The input object the event came from.
    pub input: usize,
    /// The source id.
    pub src: i32,
    /// The pad index.
    pub pad: i32,
    /// The event's property block.
    pub props: usize,
    /// The velocity.
    pub velocity: f32,
    /// Whether this record holds a real press.
    pub valid: i32,
}

/// The state the input hooks keep.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct State {
    /// Pads whose note-on the app may have seen.
    pub held_on: [u8; PADS],
    /// The input each held pad came from.
    pub held_input: [usize; PADS],
    /// The source each held pad came from.
    pub held_src: [i32; PADS],
    /// The last real press per pad.
    pub pad_rec: [PadRecord; PADS],
    /// The pad pressed last.
    pub last_pad: i32,
    /// Its property block.
    pub props: usize,
    /// Whether the mod is replaying a pad itself.
    pub replaying: i32,
    /// The control-surface object last seen.
    pub cs_self: usize,
    /// The encoder's surface, as the button handler recorded it.
    pub data_target: usize,
    /// The pad note base the app reports for the bank.
    pub pad_prop_base: i32,
}

impl State {
    /// A fresh state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            held_on: [0; PADS],
            held_input: [0; PADS],
            held_src: [0; PADS],
            pad_rec: [PadRecord {
                input: 0,
                src: 0,
                pad: 0,
                props: 0,
                velocity: 0.0,
                valid: 0,
            }; PADS],
            last_pad: -1,
            props: 0,
            replaying: 0,
            cs_self: 0,
            data_target: 0,
            pad_prop_base: 0x75,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// The app's note-on.
pub type NoteOn = unsafe extern "C" fn(usize, i32, i32, usize, f32);
/// The app's note-off.
pub type NoteOff = unsafe extern "C" fn(usize, i32, i32);
/// The app's control-surface event.
pub type ControlSurface = unsafe extern "C" fn(usize, i32, usize);
/// The app's encoder.
pub type DataSignal = unsafe extern "C" fn(usize, i32);

/// What the hooks need from the glue for one event.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context {
    /// Whether the looper view is open.
    pub looper_open: i32,
    /// Whether a spectral chop is running.
    pub spectral_active: i32,
    /// Whether the bass preview is playing.
    pub preview_active: i32,
    /// Whether the effects menu is open.
    pub menu_open: i32,
    /// Whether the bass control page is open.
    pub bass_edit: i32,
    /// Whether a job is running.
    pub busy: i32,
    /// Whether SHIFT is held.
    pub shift: i32,
    /// Whether ERASE is held.
    pub erase: i32,
    /// Redraw the screen.
    pub repaint: Option<unsafe extern "C" fn()>,
}

/// Forward a note-on to the app.
fn forward_on(
    callback: Option<NoteOn>,
    input: usize,
    src: i32,
    pad: i32,
    props: usize,
    velocity: f32,
) {
    if let Some(callback) = callback {
        // SAFETY: the caller guarantees this is the displaced hook.
        unsafe { callback(input, src, pad, props, velocity) };
    }
}

/// Forward a note-off to the app.
fn forward_off(callback: Option<NoteOff>, input: usize, src: i32, pad: i32) {
    if let Some(callback) = callback {
        // SAFETY: the caller guarantees this is the displaced hook.
        unsafe { callback(input, src, pad) };
    }
}

/// The pad index, when it is one the app can address.
fn pad_index(pad: i32) -> Option<usize> {
    usize::try_from(pad).ok().filter(|index| *index < PADS)
}

/// A pad press.
///
/// # Safety
///
/// `state` must point at the glue's state and `callback` at the app's own hook.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_input_note_on(
    state: *mut State,
    input: usize,
    src: i32,
    pad: i32,
    props: usize,
    velocity: f32,
    context: Context,
    callback: Option<NoteOn>,
) {
    if state.is_null() {
        return;
    }
    // SAFETY: the caller promises the state is live.
    let state = unsafe { &mut *state };
    if let Some(index) = pad_index(pad) {
        state.held_on[index] = 1;
        state.held_input[index] = input;
        state.held_src[index] = src;
    }
    if context.looper_open != 0 && pad >= 0 {
        if pad < LOOPER_PADS {
            crate::looper_ffi::looper_pad(pad, context.shift, context.erase);
        }
        return;
    }
    if context.spectral_active != 0 {
        return;
    }
    if context.preview_active != 0 && (0..PREVIEW_PADS).contains(&pad) {
        crate::bass_ffi::bass_preview_note(u32::try_from(pad).unwrap_or(0), velocity);
        return;
    }
    state.last_pad = pad;
    state.props = props;
    if state.replaying == 0
        && let Some(index) = pad_index(pad)
    {
        state.pad_rec[index] = PadRecord {
            input,
            src,
            pad,
            props,
            velocity,
            valid: 1,
        };
    }
    forward_on(callback, input, src, pad, props, velocity);
}

/// A pad release.
///
/// # Safety
///
/// `state` must point at the glue's state and `callback` at the app's own hook.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_input_note_off(
    state: *mut State,
    input: usize,
    src: i32,
    pad: i32,
    context: Context,
    callback: Option<NoteOff>,
) {
    if !state.is_null()
        && let Some(index) = pad_index(pad)
    {
        // SAFETY: the caller promises the state is live.
        unsafe { (*state).held_on[index] = 0 };
    }
    if context.looper_open != 0 && pad >= 0 {
        return;
    }
    if context.menu_open != 0 && context.bass_edit != 0 && (0..PREVIEW_PADS).contains(&pad) {
        return;
    }
    forward_off(callback, input, src, pad);
}

/// Release every pad the app may think is held.
///
/// # Safety
///
/// `state` must point at the glue's state and `callback` at the app's own hook.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_input_release_held(state: *mut State, callback: Option<NoteOff>) {
    if state.is_null() {
        return;
    }
    // SAFETY: the caller promises the state is live.
    let state = unsafe { &mut *state };
    for pad in 0..PADS {
        if state.held_on[pad] == 0 {
            continue;
        }
        state.held_on[pad] = 0;
        forward_off(
            callback,
            state.held_input[pad],
            state.held_src[pad],
            i32::try_from(pad).unwrap_or(0),
        );
    }
}

/// A control-surface event: remember the object and pass it on.
///
/// # Safety
///
/// `state` must point at the glue's state and `callback` at the app's own hook.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_input_surface(
    state: *mut State,
    self_address: usize,
    code: i32,
    payload: usize,
    callback: Option<ControlSurface>,
) {
    if state.is_null() {
        return;
    }
    // SAFETY: the caller promises the state is live.
    let state = unsafe { &mut *state };
    if self_address != 0 {
        state.cs_self = self_address;
    }
    if let Some(callback) = callback {
        // SAFETY: the caller guarantees this is the displaced hook.
        unsafe { callback(self_address, code, payload) };
    }
}

/// The encoder: drive the menu, or pass the event on.
///
/// # Safety
///
/// `state` must point at the glue's state and `callback` at the app's own hook.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_input_data(
    state: *mut State,
    selection: *mut i32,
    self_address: usize,
    delta: i32,
    context: Context,
    callback: Option<DataSignal>,
) {
    if state.is_null() || selection.is_null() {
        return;
    }
    // SAFETY: the caller promises the state is live.
    let data_target = unsafe { (*state).data_target };
    if self_address == data_target && (context.menu_open != 0 || context.spectral_active != 0) {
        if context.menu_open != 0 && context.busy == 0 && delta != 0 && context.bass_edit == 0 {
            let step = if delta > 0 { 1 } else { 7 };
            // The row belongs to the menu, which owns it; this hook only moves it.
            // SAFETY: the caller promises the pointer is writable.
            let current = unsafe { *selection };
            // SAFETY: see above.
            unsafe { *selection = (current + step) % MENU_ITEMS };
            if let Some(repaint) = context.repaint {
                // SAFETY: the caller's callback.
                unsafe { repaint() };
            }
        }
        return;
    }
    if let Some(callback) = callback {
        // SAFETY: the caller guarantees this is the displaced hook.
        unsafe { callback(self_address, delta) };
    }
}

/// The input state, for the C glue.
///
/// The state lives here now; the glue keeps its field names by pointing at this.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_input_state() -> *mut State {
    // Taking the address of a static needs no unsafe block; dereferencing it
    // does, and that happens in the hooks below.
    core::ptr::addr_of_mut!(STATE)
}

/// The pad pressed last, for the glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_input_last_pad() -> i32 {
    // SAFETY: see above.
    unsafe { (*core::ptr::addr_of!(STATE)).last_pad }
}

/// Remember the encoder's surface, for the glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_input_set_data_target(target: usize) {
    // SAFETY: see above.
    unsafe { (*core::ptr::addr_of_mut!(STATE)).data_target = target };
}

/// Record the encoder's surface.
pub fn set_data_target(target: usize) {
    // SAFETY: only the UI thread writes the input state.
    unsafe { (*core::ptr::addr_of_mut!(STATE)).data_target = target };
}

/// The state itself.
static mut STATE: State = State::new();

// The glue owns the screens and the keys; it builds the context for us and
// hands us its menu's selection slot.
unsafe extern "C" {
    fn minimod_glue_context() -> Context;
    fn minimod_glue_selection() -> *mut i32;
}

/// The app's own note-on, recorded at install time.
static mut ORIGINAL_ON: Option<NoteOn> = None;
/// The app's own note-off.
static mut ORIGINAL_OFF: Option<NoteOff> = None;
/// The app's own control-surface handler.
static mut ORIGINAL_SURFACE: Option<ControlSurface> = None;
/// The app's own encoder handler.
static mut ORIGINAL_DATA: Option<DataSignal> = None;

/// Record the displaced hooks, for the glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_hook_originals(
    on: Option<NoteOn>,
    off: Option<NoteOff>,
    surface: Option<ControlSurface>,
    data: Option<DataSignal>,
) {
    // SAFETY: the constructor calls this once, before any event can arrive.
    unsafe {
        ORIGINAL_ON = on;
        ORIGINAL_OFF = off;
        ORIGINAL_SURFACE = surface;
        ORIGINAL_DATA = data;
    }
}

/// A pad press, as the app's own hook.
///
/// # Safety
///
/// The app calls this with its own event pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_note_on(
    input: usize,
    src: i32,
    pad: i32,
    props: usize,
    velocity: f32,
) {
    // SAFETY: the glue builds the context from state it owns.
    let context = unsafe { minimod_glue_context() };
    // SAFETY: the state is a static in this module.
    let state = core::ptr::addr_of_mut!(STATE);
    // SAFETY: the original is the app's own hook.
    unsafe {
        minimod_input_note_on(
            state,
            input,
            src,
            pad,
            props,
            velocity,
            context,
            ORIGINAL_ON,
        );
    }
}

/// A pad release, as the app's own hook.
///
/// # Safety
///
/// The app calls this with its own event pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_note_off(input: usize, src: i32, pad: i32) {
    // SAFETY: the glue builds the context from state it owns.
    let context = unsafe { minimod_glue_context() };
    // SAFETY: the state is a static in this module.
    let state = core::ptr::addr_of_mut!(STATE);
    // SAFETY: the original is the app's own hook.
    unsafe { minimod_input_note_off(state, input, src, pad, context, ORIGINAL_OFF) };
}

/// A control-surface event, as the app's own hook.
///
/// # Safety
///
/// The app calls this with its own event pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_surface(self_address: usize, code: i32, payload: usize) {
    // SAFETY: the state is a static in this module.
    let state = core::ptr::addr_of_mut!(STATE);
    // SAFETY: the original is the app's own hook.
    unsafe { minimod_input_surface(state, self_address, code, payload, ORIGINAL_SURFACE) };
}

/// The encoder, as the app's own hook.
///
/// # Safety
///
/// The app calls this with its own event pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_data(self_address: usize, delta: i32) {
    // SAFETY: the glue builds the context from state it owns.
    let context = unsafe { minimod_glue_context() };
    // SAFETY: the state and the menu's selection are Rust statics; the glue
    // passes the menu through the context call.
    let state = core::ptr::addr_of_mut!(STATE);
    // SAFETY: the glue owns the menu and returns a writable slot for its row.
    let selection = unsafe { minimod_glue_selection() };
    // SAFETY: the state, the slot and the original are the caller's.
    unsafe {
        minimod_input_data(
            state,
            selection,
            self_address,
            delta,
            context,
            ORIGINAL_DATA,
        );
    }
}

/// Record the app's note-on handler.
///
/// # Safety
///
/// Called once at install, before any note can arrive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_note_on_original(original: usize) {
    // SAFETY: the caller's contract.
    unsafe { ORIGINAL_ON = from_address(original) };
}

/// Record the app's note-off handler.
///
/// # Safety
///
/// Called once at install, before any note can arrive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_note_off_original(original: usize) {
    // SAFETY: the caller's contract.
    unsafe { ORIGINAL_OFF = from_address(original) };
}

/// Record the app's control-surface handler.
///
/// # Safety
///
/// Called once at install, before any event can arrive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_surface_original(original: usize) {
    // SAFETY: the caller's contract.
    unsafe { ORIGINAL_SURFACE = from_address(original) };
}

/// Record the app's data-signal handler.
///
/// # Safety
///
/// Called once at install, before any event can arrive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_data_original(original: usize) {
    // SAFETY: the caller's contract.
    unsafe { ORIGINAL_DATA = from_address(original) };
}

/// One displaced function, from its address.
#[must_use]
pub fn from_address<T: Copy>(address: usize) -> Option<T> {
    if address == 0 {
        return None;
    }
    // SAFETY: the caller names the shape the site has.
    Some(unsafe { core::mem::transmute_copy::<usize, T>(&address) })
}

#[cfg(test)]
mod tests {
    use super::{MENU_ITEMS, State};

    #[test]
    fn a_fresh_state_remembers_nothing() {
        let state = State::new();
        assert_eq!(state.last_pad, -1);
        assert!(state.held_on.iter().all(|&held| held == 0));
        assert!(state.pad_rec.iter().all(|record| record.valid == 0));
        assert_eq!(state.pad_prop_base, 0x75);
        assert_eq!(MENU_ITEMS, 8);
    }
}
