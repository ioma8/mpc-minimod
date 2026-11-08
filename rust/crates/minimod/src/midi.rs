//! What a MIDI note event means to the mod.
//!
//! The event is a fixed-layout object the app dispatches: a type tag, a
//! velocity, and the note in `+0x30`.  The mod only ever needs to know whether
//! the event is a note on or off, whether it came from outside the app, and
//! which 16 Levels row it addresses - the rest of the handling stays in the glue
//! because it mutates the event and calls the app's own setters.

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

use crate::levels;

/// `[event + 8]` is the type tag, `+0x10` the velocity, `+0x30` the note.
const EVENT_TYPE: usize = 8;
const EVENT_VELOCITY: usize = 0x10;
const EVENT_NOTE: usize = 0x30;

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

/// Decode a note event into a `#[repr(C)]` struct for the glue.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawNote {
    /// The event's type tag.
    pub kind: u32,
    /// The note number.
    pub note: u32,
    /// The velocity.
    pub velocity: f32,
    /// Whether the event came from outside the app.
    pub external: i32,
    /// Whether this is a note on.
    pub note_on: i32,
    /// Whether this is a note off.
    pub note_off: i32,
    /// The 16 Levels row.
    pub row: i32,
}

/// Decode a note event for the C glue.
///
/// # Safety
///
/// `event` must be readable for `EVENT_NOTE + 1` bytes and `out` must be
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_midi_decode(
    event: *const u8,
    source: u64,
    out: *mut RawNote,
) -> i32 {
    if event.is_null() || out.is_null() {
        return 0;
    }
    // SAFETY: the caller promises both pointers are valid.
    let bytes = unsafe { core::slice::from_raw_parts(event, EVENT_NOTE + 1) };
    let note = decode(bytes, source);
    // SAFETY: the caller promises `out` is writable.
    unsafe {
        *out = RawNote {
            kind: u32::from(note.kind),
            note: u32::from(note.note),
            velocity: note.velocity,
            external: i32::from(note.external),
            note_on: i32::from(note.note_on),
            note_off: i32::from(note.note_off),
            row: note.row,
        };
    }
    1
}

/// The event fields the replay rewrites, so the caller can put them back.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Saved {
    /// The two property-mask bytes at `+0x14`.
    pub mask: u16,
    /// The property value at `+0x16`.
    pub property: u8,
    /// The note at `+0x30`.
    pub note: u8,
}

/// `[event + 0x14]` is the property mask, `+0x16` its value, `+0x30` the note.
const EVENT_MASK: usize = 0x14;
const EVENT_PROPERTY: usize = 0x16;

/// Re-send a note as the selected pad's event.
///
/// The event is rewritten in place: the property mask is cleared, the selected
/// pad goes into `+0x30`, and the pitch offset is applied through the app's own
/// setter - the same call the 16 Levels renderer makes.  The caller forwards the
/// event and then restores it.
///
/// # Safety
///
/// `event` must be writable for `EVENT_NOTE + 1` bytes, and `pitch_off` must be
/// the app's displaced setter when present.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_midi_replay(
    event: *mut u8,
    pad_level_byte: u8,
    row: i32,
    pitch_off: Option<levels::PitchOffset>,
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

/// Put back what [`minimod_midi_replay`] changed.
///
/// # Safety
///
/// `event` must be the same writable event the replay was given.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_midi_restore(event: *mut u8, saved: Saved) {
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

#[cfg(test)]
mod tests {
    use super::{Note, decode};

    fn event(kind: u8, velocity: f32, note: u8) -> [u8; 0x31] {
        let mut bytes = [0_u8; 0x31];
        bytes[8] = kind;
        bytes[0x10..0x14].copy_from_slice(&velocity.to_le_bytes());
        bytes[0x30] = note;
        bytes
    }

    fn external_source() -> u64 {
        // The alternate external id the firmware uses.
        0x7fff_fff6_u64
    }

    #[test]
    fn a_pad_press_with_velocity_is_a_note_on() {
        let note: Note = decode(&event(3, 0.8, 72), external_source());
        assert!(note.note_on);
        assert!(!note.note_off);
        assert!(note.external);
        assert_eq!(note.note, 72);
        assert_eq!(note.row, 12);
        assert!((note.velocity - 0.8).abs() < 1e-6);
    }

    #[test]
    fn a_pad_release_is_a_note_off() {
        let note = decode(&event(3, 0.0, 60), external_source());
        assert!(note.note_off);
        assert!(!note.note_on);
        assert_eq!(note.row, 0);
    }

    #[test]
    fn a_type_two_event_is_a_note_off_even_with_velocity() {
        let note = decode(&event(2, 0.5, 65), external_source());
        assert!(note.note_off, "type 2 is an explicit note off");
        assert!(!note.note_on);
        // And a note above 127 is never a note off.
        let high = decode(&event(2, 0.0, 200), external_source());
        assert!(!high.note_off);
    }

    #[test]
    fn only_the_two_external_source_ids_count_as_external() {
        assert!(
            decode(&event(3, 1.0, 60), 0x7fff_ffef).external,
            "the limit"
        );
        assert!(
            decode(&event(3, 1.0, 60), 0x7fff_fff6).external,
            "the alternate id"
        );
        assert!(
            decode(&event(3, 1.0, 60), 0x8000_0000).external,
            "signed compare"
        );
        assert!(!decode(&event(3, 1.0, 60), 0x7fff_fff0).external);
        assert!(
            !decode(&event(3, 1.0, 60), 0x7fff_fffa).external,
            "the synth id"
        );
        assert!(
            decode(&event(3, 1.0, 60), 0x10).external,
            "small ids are external"
        );
    }
}
