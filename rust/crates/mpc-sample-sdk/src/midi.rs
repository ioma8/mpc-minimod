//! MIDI note events: decoding, the 16 Levels row, and replaying a note as the
//! selected pad.
//!
//! The event is a fixed-layout object the app dispatches: a type tag, a
//! velocity, and the note at `+0x30`.  The SDK decodes it and can rewrite it in
//! place; which notes a mod replays is policy and lives in the mod.

/// The event's type tag for a pad/note event.
pub const PAD_EVENT: u8 = 3;
/// The event's type tag for a note off.
pub const NOTE_OFF_EVENT: u8 = 2;
/// The note 16 Levels treats as its first row.
pub const ROW_ORIGIN: i32 = 60;
/// Source ids at or below this are "outside the app" (compared as signed, so the
/// high ids the app uses internally are excluded).
pub const EXTERNAL_SOURCE_LIMIT: i32 = 0x7fff_ffef;
/// A second external source id the firmware uses.
pub const EXTERNAL_SOURCE_ALT: i32 = 0x7fff_fff6;
/// The source id a replayed event carries, which the mod's own hook filters out.
pub const REPLAY_SOURCE: u64 = 0x8000_0000;

/// The app's 16 Levels pitch setter.
pub type PitchOffset = unsafe extern "C" fn(usize, u64, u64, i32);

/// `[event + 8]` is the type tag, `+0x10` the velocity, `+0x30` the note.
const EVENT_TYPE: usize = 8;
const EVENT_VELOCITY: usize = 0x10;
const EVENT_NOTE: usize = 0x30;
/// `[event + 0x14]` is the property mask, `+0x16` its value.
const EVENT_MASK: usize = 0x14;
const EVENT_PROPERTY: usize = 0x16;

/// A decoded note event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    /// The event's type tag, low byte.
    pub kind: u8,
    /// The note number, or 0xff when the event carries none.
    pub note: u8,
    /// The velocity.
    pub velocity: f32,
    /// Whether the event came from outside the app.
    pub external: bool,
    /// Whether this is a note on.
    pub note_on: bool,
    /// Whether this is a note off.
    pub note_off: bool,
    /// The 16 Levels row this note addresses.
    pub row: i32,
}

/// Decode a note event.
///
/// `event` must be at least `EVENT_NOTE + 1` bytes; a shorter slice is treated
/// as an event with no note, which is what the caller's bounds check already
/// guarantees in practice.
#[must_use]
pub fn decode(event: &[u8], source: u64) -> Note {
    let kind = event.get(EVENT_TYPE).copied().unwrap_or(0);
    let velocity = event
        .get(EVENT_VELOCITY..EVENT_VELOCITY + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map_or(0.0, f32::from_le_bytes);
    let note = event.get(EVENT_NOTE).copied().unwrap_or(0xff);
    let source_id = (source as u32).cast_signed();
    let external = source_id <= EXTERNAL_SOURCE_LIMIT || source_id == EXTERNAL_SOURCE_ALT;
    let note_on = kind == PAD_EVENT && velocity > 0.0;
    let note_off =
        note <= 127 && ((kind == PAD_EVENT && velocity <= 0.0) || kind == NOTE_OFF_EVENT);
    Note {
        kind,
        note,
        velocity,
        external,
        note_on,
        note_off,
        row: i32::from(note) - ROW_ORIGIN,
    }
}

/// The event fields the replay rewrites, so it can put them back.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Saved {
    mask: u16,
    property: u8,
    note: u8,
}

/// Re-send a note as the selected pad's event.
///
/// The event is rewritten in place: the property mask is cleared, the selected
/// pad goes into `+0x30`, and the pitch offset is applied through the app's own
/// setter - the same call the 16 Levels renderer makes.
///
/// # Safety
///
/// `event` must be writable for `EVENT_NOTE + 1` bytes, and `pitch_off` must be
/// the app's displaced setter when present.
unsafe fn replay(
    event: *mut u8,
    pad_level_byte: u8,
    row: i32,
    pitch_off: Option<PitchOffset>,
) -> Saved {
    let mut saved = Saved {
        mask: 0,
        property: 0,
        note: 0xff,
    };
    if event.is_null() {
        return saved;
    }
    // SAFETY: the caller promises the event is writable for the fields below.
    unsafe {
        saved.mask = core::ptr::read_unaligned(event.add(EVENT_MASK).cast::<u16>());
        saved.property = *event.add(EVENT_PROPERTY);
        saved.note = *event.add(EVENT_NOTE);
        core::ptr::write_unaligned(event.add(EVENT_MASK).cast::<u16>(), 0);
        *event.add(EVENT_NOTE) = pad_level_byte;
    }
    if let Some(setter) = pitch_off {
        // SAFETY: the caller guarantees this is the app's setter, and the
        // property triple it expects starts at `+0x14`.
        unsafe {
            setter(event.wrapping_add(EVENT_MASK) as usize, 0, 0, row);
        }
    }
    saved
}

/// Put back what [`replay`] changed.
///
/// # Safety
///
/// `event` must be the same writable event the replay was given.
unsafe fn restore(event: *mut u8, saved: Saved) {
    if event.is_null() {
        return;
    }
    // SAFETY: the caller promises the event is writable for these fields.
    unsafe {
        core::ptr::write_unaligned(event.add(EVENT_MASK).cast::<u16>(), saved.mask);
        *event.add(EVENT_PROPERTY) = saved.property;
        *event.add(EVENT_NOTE) = saved.note;
    }
}

/// One note event, decoded and ready to be replayed.
///
/// The mod reads the fields, optionally calls [`NoteEvent::replay_as_pad`], and
/// returns [`crate::NoteFlow`]; the SDK forwards and restores the event.
pub struct NoteEvent {
    /// The app's `this` pointer.
    pub app: usize,
    /// The context the app passed with the event.
    pub context: usize,
    /// The app's event object, writable while the hook runs.
    pub event: *mut u8,
    /// The source id the app passed.
    pub source: u64,
    /// The event's type tag.
    pub kind: u8,
    /// The note number, or 0xff.
    pub note: u8,
    /// The velocity.
    pub velocity: f32,
    /// Whether the note came from outside the app.
    pub external: bool,
    /// Whether this is a note on.
    pub note_on: bool,
    /// Whether this is a note off.
    pub note_off: bool,
    /// The 16 Levels row the note addresses.
    pub row: i32,
    /// The event's property mask at `+0x14`.
    pub mask: u16,
    /// The event's property value at `+0x16`.
    pub property: u8,
    /// The context to forward with; `context` unless replayed.
    forward_context: usize,
    /// The source to forward with; `source` unless replayed.
    forward_source: u64,
    /// What the replay rewrote, while one is armed.
    saved: Option<Saved>,
    /// The app's pitch setter, when the site has one.
    pitch_off: Option<PitchOffset>,
}

impl NoteEvent {
    /// Decode the event behind `event`.
    ///
    /// # Safety
    ///
    /// `event` must be readable for `EVENT_NOTE + 1` bytes or null.
    #[must_use]
    pub unsafe fn new(
        app: usize,
        context: usize,
        event: *mut u8,
        source: u64,
        pitch_off: Option<PitchOffset>,
    ) -> Self {
        let bytes = if event.is_null() {
            &[][..]
        } else {
            // SAFETY: the caller promises readability for these bytes.
            unsafe { core::slice::from_raw_parts(event, EVENT_NOTE + 1) }
        };
        let note = decode(bytes, source);
        // SAFETY: the caller promises readability for the whole event.
        let (mask, property) = if event.is_null() {
            (0, 0)
        } else {
            // SAFETY: the caller promised the event is readable for these fields.
            unsafe {
                (
                    core::ptr::read_unaligned(event.add(EVENT_MASK).cast::<u16>()),
                    *event.add(EVENT_PROPERTY),
                )
            }
        };
        Self {
            app,
            context,
            event,
            source,
            kind: note.kind,
            note: note.note,
            velocity: note.velocity,
            external: note.external,
            note_on: note.note_on,
            note_off: note.note_off,
            row: note.row,
            mask,
            property,
            forward_context: context,
            forward_source: source,
            saved: None,
            pitch_off,
        }
    }

    /// Rewrite the event as `pad` at `row`, forwarded in `context`.
    ///
    /// Returns `false` when the event is not writable, in which case the caller
    /// must fall back to a plain forward.
    pub fn replay_as_pad(&mut self, pad: u8, row: i32, context: usize) -> bool {
        if self.event.is_null() {
            return false;
        }
        // SAFETY: the site promises the event is writable for the hook.
        self.saved = Some(unsafe { replay(self.event, pad, row, self.pitch_off) });
        self.forward_context = context;
        self.forward_source = REPLAY_SOURCE;
        true
    }

    /// The context the SDK forwards with.
    #[must_use]
    pub const fn forward_context(&self) -> usize {
        self.forward_context
    }

    /// The source id the SDK forwards with.
    #[must_use]
    pub const fn forward_source(&self) -> u64 {
        self.forward_source
    }

    /// Put the event back the way the app wrote it.
    pub fn restore(&mut self) {
        if let Some(saved) = self.saved.take() {
            // SAFETY: the saved fields belong to the same event.
            unsafe { restore(self.event, saved) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Note, NoteEvent, REPLAY_SOURCE, ROW_ORIGIN, decode};

    fn event(kind: u8, velocity: f32, note: u8) -> [u8; 0x40] {
        let mut bytes = [0_u8; 0x40];
        bytes[8] = kind;
        bytes[0x10..0x14].copy_from_slice(&velocity.to_le_bytes());
        bytes[0x30] = note;
        bytes
    }

    #[test]
    fn a_pad_event_decodes_a_press_and_a_release() {
        let press = decode(&event(3, 0.8, 64), 0x10);
        assert!(press.note_on && !press.note_off);
        assert_eq!(press.row, 64 - ROW_ORIGIN);
        let release = decode(&event(3, 0.0, 64), 0x10);
        assert!(!release.note_on && release.note_off);
    }

    #[test]
    fn a_replayed_event_restores_the_original_bytes() {
        let mut bytes = event(3, 0.8, 64);
        // SAFETY: the buffer is a whole event.
        let mut note = unsafe { NoteEvent::new(0, 0, bytes.as_mut_ptr(), 0, None) };
        assert_eq!(note.note, 64);
        assert!(note.replay_as_pad(7, 3, 0x1000));
        assert_eq!(bytes[0x30], 7);
        assert_eq!(note.forward_source(), REPLAY_SOURCE);
        note.restore();
        assert_eq!(bytes[0x30], 64);
    }

    #[test]
    fn a_short_event_decodes_as_no_note() {
        assert_eq!(
            decode(&[], 0),
            Note {
                kind: 0,
                note: 0xff,
                velocity: 0.0,
                external: true,
                note_on: false,
                note_off: false,
                row: 0xff - ROW_ORIGIN,
            }
        );
    }
}
