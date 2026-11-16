//! External MIDI chromatic playback while 16 Levels is active.
//!
//! The SDK decodes the app's note event and forwards it; [`note`] decides when
//! to replay it as the selected pad. The note hook runs on the live input path,
//! without the UI module lock, so every field the UI also touches is atomic and
//! the press/release pairing lives in a fixed-capacity atomic ledger. A release
//! therefore never loses the press it belongs to, even when another callback
//! holds the module lock.

use core::sync::atomic::{AtomicI32, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use mpc_sample_sdk::{Flow, NoteEvent};

/// The event's type tag for a pad/note event.
pub const PAD_EVENT: u8 = 3;

/// How many presses of the same note can be replayed chromatically.
const PER_NOTE: usize = 4;

/// No press owns this slot.
const FREE: u64 = 0;
/// A press is publishing its target.
const WRITING: u64 = 1;
/// The note is held; its release replays the same pad row.
const HELD: u64 = 2;
/// A release arrived while the press was publishing, so the press is refused.
const ABORTED: u64 = 3;
/// The low byte of a slot state holds its stage.
const STAGE_MASK: u64 = 0xff;

/// A published slot value: the press order plus the stage.
const fn tagged(sequence: u64, stage: u64) -> u64 {
    (sequence << 8) | stage
}

/// What the MIDI policy carries between events.
///
/// The device test reads `g_midi` by offset, so the field order and the `i32`
/// layout are fixed: the dispatcher flag, the 16 Levels flag, the pad context,
/// the learned pad properties, the per-note replay flags and the tail counters.
/// The note hook and the UI thread both touch the struct, so every field is an
/// atomic with the same size as the value it holds.
#[repr(C)]
pub struct MidiState {
    /// Whether the mod is handling notes at all.
    pub on: AtomicI32,
    /// Whether 16 Levels is active.
    pub levels: AtomicI32,
    /// The context of the pad that last sent an event.
    pub pad_ctx: AtomicUsize,
    /// The selected pad's note byte.
    pub pad_level_byte: AtomicU32,
    /// Whether the pad is in tune mode.
    pub pad_tune_mode: AtomicI32,
    /// The property value the pad reported.
    pub pad_prop_val: AtomicI32,
    /// Whether that property is waiting to be learned.
    pub pad_prop_pending: AtomicI32,
    /// Notes the mod is holding open, so a release can be matched.
    pub replayed: [AtomicU8; 128],
    /// How many notes the mod has replayed, for diagnostics and tests.
    pub chromatic_count: AtomicU32,
    /// The last note it replayed.
    pub chromatic_note: AtomicI32,
    /// That note's row.
    pub chromatic_delta: AtomicI32,
}

impl MidiState {
    /// The empty state every process starts with.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            on: AtomicI32::new(0),
            levels: AtomicI32::new(0),
            pad_ctx: AtomicUsize::new(0),
            pad_level_byte: AtomicU32::new(0),
            pad_tune_mode: AtomicI32::new(0),
            pad_prop_val: AtomicI32::new(0),
            pad_prop_pending: AtomicI32::new(0),
            replayed: [const { AtomicU8::new(0) }; 128],
            chromatic_count: AtomicU32::new(0),
            chromatic_note: AtomicI32::new(0),
            chromatic_delta: AtomicI32::new(0),
        }
    }
}

impl Default for MidiState {
    fn default() -> Self {
        Self::new()
    }
}

mpc_sample_sdk::export_static!(#[doc = "The MIDI state, read by the device tests."]
g_midi: MidiState = MidiState::new());

/// One replayed note and the pad row that has to be released with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Target {
    pad: u8,
    row: i32,
    context: usize,
}

/// One press slot; `note` is the slot's index, not a field.
struct NoteSlot {
    state: AtomicU64,
    source: AtomicU64,
    pad: AtomicU8,
    row: AtomicI32,
    context: AtomicUsize,
}

impl NoteSlot {
    const fn new() -> Self {
        Self {
            state: AtomicU64::new(FREE),
            source: AtomicU64::new(0),
            pad: AtomicU8::new(0),
            row: AtomicI32::new(0),
            context: AtomicUsize::new(0),
        }
    }
}

/// What a release decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    /// The note was not replayed: let the app handle the release.
    Unmatched,
    /// The app never received the press, so it must not receive the release.
    Consume,
    /// Replay the release as this pad row.
    Replay(Target),
}

/// Fixed-capacity ownership of replayed MIDI `(source, note)` pairs.
///
/// Each note owns a few slots so overlapping sources keep their own targets.
/// Presses never allocate or wait; at capacity, new presses use native playback.
struct NoteLedger {
    slots: [[NoteSlot; PER_NOTE]; 128],
    next: AtomicU64,
    native: [AtomicU32; 128],
}

impl NoteLedger {
    #[allow(
        clippy::large_stack_arrays,
        reason = "The production ledger is static and never allocated in a callback"
    )]
    const fn new() -> Self {
        Self {
            slots: [const { [const { NoteSlot::new() }; PER_NOTE] }; 128],
            next: AtomicU64::new(0),
            native: [const { AtomicU32::new(0) }; 128],
        }
    }

    /// Remember one replayed press. False when the note is at capacity or a
    /// release arrived while it was publishing.
    #[cfg(test)]
    fn hold(&self, source: u64, note: u8, target: Target) -> bool {
        self.reserve(source, note, target).unwrap_or(false)
    }

    // None means capacity exhaustion; Some(false) means an early release.
    fn reserve(&self, source: u64, note: u8, target: Target) -> Option<bool> {
        let slots = self.slots.get(usize::from(note))?;
        let Ok(sequence) = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                (next < (u64::MAX >> 8)).then_some(next + 1)
            })
        else {
            return None;
        };
        let writing = tagged(sequence, WRITING);
        for slot in slots {
            if slot
                .state
                .compare_exchange(FREE, writing, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            slot.source.store(source, Ordering::Relaxed);
            slot.pad.store(target.pad, Ordering::Relaxed);
            slot.row.store(target.row, Ordering::Relaxed);
            slot.context.store(target.context, Ordering::Relaxed);
            if slot
                .state
                .compare_exchange(
                    writing,
                    tagged(sequence, HELD),
                    Ordering::Release,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Some(true);
            }
            // A release aborted the press before it became visible; it also
            // consumed the release, so neither edge may be forwarded.
            let _ = slot
                .state
                .compare_exchange(writing, FREE, Ordering::AcqRel, Ordering::Acquire);
            let _ = slot.state.compare_exchange(
                tagged(sequence, ABORTED),
                FREE,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            return Some(false);
        }
        None
    }

    /// Claim the oldest held press for `(source, note)`, or refuse a press that
    /// is still publishing rather than forward an unmatched release.
    fn release(&self, source: u64, note: u8) -> Claim {
        let Some(slots) = self.slots.get(usize::from(note)) else {
            return Claim::Unmatched;
        };
        for _ in 0..(PER_NOTE * 2) {
            let mut oldest: Option<(usize, u64)> = None;
            for (index, slot) in slots.iter().enumerate() {
                let state = slot.state.load(Ordering::Acquire);
                if state & STAGE_MASK != HELD || slot.source.load(Ordering::Relaxed) != source {
                    continue;
                }
                let sequence = state >> 8;
                if oldest.is_none_or(|(_, previous)| sequence < previous) {
                    oldest = Some((index, sequence));
                }
            }
            let Some((index, _)) = oldest else {
                break;
            };
            let slot = &slots[index];
            let state = slot.state.load(Ordering::Acquire);
            if state & STAGE_MASK != HELD || slot.source.load(Ordering::Relaxed) != source {
                continue;
            }
            let target = Target {
                pad: slot.pad.load(Ordering::Relaxed),
                row: slot.row.load(Ordering::Relaxed),
                context: slot.context.load(Ordering::Relaxed),
            };
            if slot
                .state
                .compare_exchange(state, FREE, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Claim::Replay(target);
            }
        }
        for slot in slots {
            let state = slot.state.load(Ordering::Acquire);
            if state & STAGE_MASK != WRITING {
                continue;
            }
            // Metadata is not published while WRITING, including on reused
            // slots. Conservatively cancel any overlapping unpublished press.
            if slot
                .state
                .compare_exchange(
                    state,
                    tagged(state >> 8, ABORTED),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Claim::Consume;
            }
        }
        Claim::Unmatched
    }

    /// Whether any source still holds this note.
    fn is_held(&self, note: u8) -> bool {
        self.slots.get(usize::from(note)).is_some_and(|slots| {
            slots
                .iter()
                .any(|slot| slot.state.load(Ordering::Acquire) & STAGE_MASK == HELD)
        })
    }
}

/// The owned target ledger, touched only from the note hook.
static TARGETS: NoteLedger = NoteLedger::new();

/// The parts of one note event the policy reads.
///
/// Copied out of [`NoteEvent`] so the decision itself is testable and only the
/// replay touches the borrowed app event.
#[derive(Clone, Copy, Debug)]
struct NoteFacts {
    kind: u8,
    number: u8,
    velocity: f32,
    external: bool,
    on: bool,
    off: bool,
    mask: u16,
    property: u8,
    context: usize,
    source: u64,
}

/// What the policy decided for one note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Forward,
    Consume,
    Replay(Target),
}

/// Handle one note event; the SDK forwards whatever this returns.
pub fn note(event: &mut NoteEvent<'_>, last_pad: i32) -> Flow {
    let facts = NoteFacts {
        kind: event.kind,
        number: event.note,
        velocity: event.velocity,
        external: event.external,
        on: event.note_on,
        off: event.note_off,
        mask: event.mask,
        property: event.property,
        context: event.context,
        source: event.source,
    };
    match decide(&g_midi, &TARGETS, facts, last_pad) {
        Decision::Forward => Flow::Forward,
        Decision::Consume => Flow::Consume,
        Decision::Replay(target) => {
            let _ = event.replay_as_pad(target.pad, target.row, target.context);
            Flow::Forward
        }
    }
}

/// Decide one note against the published mode and the owned ledger.
fn decide(state: &MidiState, targets: &NoteLedger, facts: NoteFacts, last_pad: i32) -> Decision {
    // Learn the selected pad's context even while the mod is off, so entering
    // 16 Levels after selecting a pad can still replay its notes.
    if !facts.external && facts.kind == PAD_EVENT && facts.context != 0 {
        state.pad_ctx.store(facts.context, Ordering::Release);
    }
    if state.on.load(Ordering::Acquire) == 0 && !facts.external {
        return Decision::Forward;
    }
    if !facts.external && facts.kind == PAD_EVENT && facts.velocity > 0.0 {
        state
            .pad_tune_mode
            .store(i32::from(facts.mask & 1 == 0), Ordering::Release);
        if facts.mask & 1 != 0 {
            state
                .pad_prop_val
                .store(i32::from(facts.property), Ordering::Release);
            state
                .pad_level_byte
                .store(u32::from(facts.number), Ordering::Release);
            state.pad_prop_pending.store(1, Ordering::Release);
        }
    }
    if !facts.external || facts.number > 127 || (facts.kind != PAD_EVENT && !facts.off) {
        return Decision::Forward;
    }
    if facts.off {
        return match targets.release(facts.source, facts.number) {
            Claim::Replay(target) => {
                publish_replayed(state, targets, facts.number);
                Decision::Replay(target)
            }
            Claim::Consume => Decision::Consume,
            Claim::Unmatched => {
                let _ = targets.native[usize::from(facts.number)].fetch_update(
                    Ordering::AcqRel,
                    Ordering::Acquire,
                    |count| count.checked_sub(1),
                );
                Decision::Forward
            }
        };
    }
    if !facts.on {
        return Decision::Forward;
    }
    let context = state.pad_ctx.load(Ordering::Acquire);
    if state.on.load(Ordering::Acquire) == 0
        || state.levels.load(Ordering::Acquire) == 0
        || context == 0
    {
        targets.native[usize::from(facts.number)].fetch_add(1, Ordering::AcqRel);
        return Decision::Forward;
    }
    // Do not accept a newer chromatic press ahead of an outstanding native
    // overflow press: releases for repeated notes are paired FIFO.
    let native = &targets.native[usize::from(facts.number)];
    if native.load(Ordering::Acquire) != 0 {
        native.fetch_add(1, Ordering::AcqRel);
        return Decision::Forward;
    }
    let target = Target {
        pad: u8::try_from(state.pad_level_byte.load(Ordering::Acquire)).unwrap_or(0),
        row: i32::from(facts.number) - crate::autotrim::pad_root(last_pad),
        context,
    };
    match targets.reserve(facts.source, facts.number, target) {
        Some(true) => {}
        Some(false) => return Decision::Consume,
        // Preserve the native press/release pair when chromatic ownership is
        // full. Releases drain older chromatic presses first (FIFO).
        None => {
            native.fetch_add(1, Ordering::AcqRel);
            return Decision::Forward;
        }
    }
    state.chromatic_count.fetch_add(1, Ordering::Relaxed);
    state
        .chromatic_note
        .store(i32::from(facts.number), Ordering::Relaxed);
    state.chromatic_delta.store(target.row, Ordering::Relaxed);
    publish_replayed(state, targets, facts.number);
    Decision::Replay(target)
}

/// Publish whether the note is still held, for the device test.
fn publish_replayed(state: &MidiState, targets: &NoteLedger, note: u8) {
    let index = usize::from(note);
    if let Some(slot) = state.replayed.get(index) {
        slot.store(u8::from(targets.is_held(note)), Ordering::Release);
    }
}

/// Whether the mod handles notes at all.
///
/// Turning the mode off also drops 16 Levels: `ui` calls this with the value
/// `levels_active` returned, so a page that went away cannot leave a mode that
/// reappears when it comes back.
pub fn set_on(on: i32) {
    let on = i32::from(on != 0);
    g_midi.on.store(on, Ordering::Release);
    if on == 0 {
        g_midi.levels.store(0, Ordering::Release);
    }
}

/// Toggle 16 Levels and keep the dispatcher in step.
pub fn toggle_levels(last_pad: i32) {
    let pad = last_pad;
    let levels = g_midi.levels.load(Ordering::Acquire);
    let next = i32::from(levels == 0);
    if next != 0 && (0..16).contains(&pad) {
        g_midi
            .pad_level_byte
            .store(u32::try_from(pad).unwrap_or(0), Ordering::Release);
    }
    g_midi.levels.store(next, Ordering::Release);
    g_midi.on.store(next, Ordering::Release);
}

/// Whether 16 Levels is showing and its page predicate agrees.
#[must_use]
pub fn levels_active() -> i32 {
    let screen = crate::paint::screen();
    if screen == 0 {
        return 0;
    }
    // SAFETY: `screen` is the component the paint hook last recorded on this
    // UI thread.
    let visible = unsafe { crate::page::visible(screen) };
    if !crate::page::is_sample_page(screen) || !visible {
        return 0;
    }
    g_midi.levels.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::{Claim, Decision, FREE, MidiState, NoteFacts, NoteLedger, Target, decide};
    use core::sync::atomic::Ordering;

    fn note(source: u64, number: u8, on: bool) -> NoteFacts {
        NoteFacts {
            kind: super::PAD_EVENT,
            number,
            velocity: if on { 0.8 } else { 0.0 },
            external: true,
            on,
            off: !on,
            mask: 0,
            property: 0,
            context: 0x1000,
            source,
        }
    }

    /// The replay a decision must have produced, or an impossible target that
    /// fails the following assertion.
    fn replayed(decision: Decision) -> Target {
        match decision {
            Decision::Replay(target) => target,
            Decision::Forward | Decision::Consume => Target {
                pad: u8::MAX,
                row: i32::MAX,
                context: 0,
            },
        }
    }

    fn levels_on(state: &MidiState) {
        state.on.store(1, Ordering::Release);
        state.levels.store(1, Ordering::Release);
        state.pad_ctx.store(0x1000, Ordering::Release);
        state.pad_level_byte.store(4, Ordering::Release);
    }

    #[test]
    fn a_replayed_press_and_its_release_share_one_target() {
        let state = MidiState::new();
        let targets = NoteLedger::new();
        levels_on(&state);
        let press = decide(&state, &targets, note(7, 60, true), 4);
        let target = replayed(press);
        assert_eq!(target.pad, 4);
        assert_eq!(state.replayed[60].load(Ordering::Acquire), 1);
        let release = decide(&state, &targets, note(7, 60, false), 4);
        assert_eq!(release, Decision::Replay(target));
        assert_eq!(state.replayed[60].load(Ordering::Acquire), 0);
        assert_eq!(targets.release(7, 60), Claim::Unmatched);
    }

    #[test]
    fn overlapping_sources_release_in_press_order() {
        let state = MidiState::new();
        let targets = NoteLedger::new();
        levels_on(&state);
        state.pad_level_byte.store(4, Ordering::Release);
        let first = decide(&state, &targets, note(1, 64, true), 4);
        state.pad_level_byte.store(5, Ordering::Release);
        let second = decide(&state, &targets, note(2, 64, true), 4);
        assert_ne!(first, second);
        assert_eq!(decide(&state, &targets, note(2, 64, false), 4), second);
        assert_eq!(decide(&state, &targets, note(1, 64, false), 4), first);
    }

    #[test]
    fn repeated_presses_from_one_source_release_the_oldest_first() {
        let state = MidiState::new();
        let targets = NoteLedger::new();
        levels_on(&state);
        state.pad_level_byte.store(4, Ordering::Release);
        let first = decide(&state, &targets, note(3, 64, true), 4);
        state.pad_level_byte.store(5, Ordering::Release);
        let second = decide(&state, &targets, note(3, 64, true), 4);
        assert_eq!(decide(&state, &targets, note(3, 64, false), 4), first);
        assert_eq!(decide(&state, &targets, note(3, 64, false), 4), second);
    }

    #[test]
    fn a_release_before_the_press_is_published_consumes_both_edges() {
        let ledger = NoteLedger::new();
        let slot = &ledger.slots[60][0];
        slot.state
            .store(super::tagged(9, super::WRITING), Ordering::Release);
        slot.source.store(3, Ordering::Release);
        assert_eq!(ledger.release(3, 60), Claim::Consume);
        assert_eq!(
            slot.state.load(Ordering::Acquire),
            super::tagged(9, super::ABORTED)
        );
    }

    #[test]
    fn reused_slot_metadata_cannot_hide_an_early_release() {
        let ledger = NoteLedger::new();
        let slot = &ledger.slots[60][0];
        slot.source.store(99, Ordering::Relaxed);
        slot.state
            .store(super::tagged(10, super::WRITING), Ordering::Release);
        assert_eq!(ledger.release(7, 60), Claim::Consume);
        assert_eq!(
            slot.state.load(Ordering::Acquire),
            super::tagged(10, super::ABORTED)
        );
    }

    #[test]
    fn capacity_overflow_forwards_both_native_edges() {
        let state = MidiState::new();
        let ledger = NoteLedger::new();
        levels_on(&state);
        for source in 0..super::PER_NOTE as u64 {
            assert!(matches!(
                decide(&state, &ledger, note(source, 60, true), 4),
                Decision::Replay(_)
            ));
        }
        assert_eq!(
            decide(&state, &ledger, note(99, 60, true), 4),
            Decision::Forward
        );
        assert_eq!(
            decide(&state, &ledger, note(99, 60, false), 4),
            Decision::Forward
        );
        for source in 0..super::PER_NOTE as u64 {
            assert!(matches!(
                decide(&state, &ledger, note(source, 60, false), 4),
                Decision::Replay(_)
            ));
        }
    }

    #[test]
    fn newer_presses_stay_native_until_overflow_releases_drain() {
        let state = MidiState::new();
        let ledger = NoteLedger::new();
        levels_on(&state);
        for _ in 0..super::PER_NOTE {
            assert!(matches!(
                decide(&state, &ledger, note(1, 60, true), 4),
                Decision::Replay(_)
            ));
        }
        assert_eq!(
            decide(&state, &ledger, note(1, 60, true), 4),
            Decision::Forward
        );
        for _ in 0..super::PER_NOTE {
            assert!(matches!(
                decide(&state, &ledger, note(1, 60, false), 4),
                Decision::Replay(_)
            ));
        }
        assert_eq!(
            decide(&state, &ledger, note(1, 60, true), 4),
            Decision::Forward
        );
        for _ in 0..2 {
            assert_eq!(
                decide(&state, &ledger, note(1, 60, false), 4),
                Decision::Forward
            );
        }
        assert!(matches!(
            decide(&state, &ledger, note(1, 60, true), 4),
            Decision::Replay(_)
        ));
    }

    #[test]
    fn native_notes_keep_their_releases_across_mode_changes() {
        let state = MidiState::new();
        let ledger = NoteLedger::new();
        assert_eq!(
            decide(&state, &ledger, note(1, 60, true), 4),
            Decision::Forward
        );
        levels_on(&state);
        assert_eq!(
            decide(&state, &ledger, note(1, 60, true), 4),
            Decision::Forward
        );
        state.on.store(0, Ordering::Release);
        assert_eq!(
            decide(&state, &ledger, note(1, 60, true), 4),
            Decision::Forward
        );
        levels_on(&state);
        for _ in 0..3 {
            assert_eq!(
                decide(&state, &ledger, note(1, 60, false), 4),
                Decision::Forward
            );
        }
        assert!(matches!(
            decide(&state, &ledger, note(1, 60, true), 4),
            Decision::Replay(_)
        ));
    }

    #[test]
    fn a_full_note_refuses_new_presses_but_keeps_the_held_ones() {
        let ledger = NoteLedger::new();
        let target = Target {
            pad: 1,
            row: 0,
            context: 0x1000,
        };
        for source in 0..super::PER_NOTE as u64 {
            assert!(ledger.hold(source, 60, target));
        }
        assert!(!ledger.hold(99, 60, target));
        for source in 0..super::PER_NOTE as u64 {
            assert_eq!(ledger.release(source, 60), Claim::Replay(target));
        }
        assert!(ledger.hold(99, 60, target));
    }

    #[test]
    fn a_release_for_an_unknown_source_leaves_the_held_note_alone() {
        let ledger = NoteLedger::new();
        let target = Target {
            pad: 2,
            row: 5,
            context: 0x2000,
        };
        assert!(ledger.hold(1, 61, target));
        assert_eq!(ledger.release(2, 61), Claim::Unmatched);
        assert!(ledger.is_held(61));
        assert_eq!(ledger.release(1, 61), Claim::Replay(target));
        assert!(!ledger.is_held(61));
    }

    #[test]
    fn a_physical_pad_press_learns_the_context_while_the_mod_is_off() {
        let state = MidiState::new();
        let targets = NoteLedger::new();
        let mut event = note(0, 60, true);
        event.external = false;
        event.context = 0x3000;
        assert_eq!(decide(&state, &targets, event, 4), Decision::Forward);
        assert_eq!(state.pad_ctx.load(Ordering::Acquire), 0x3000);
    }

    #[test]
    fn a_release_without_a_replayed_press_is_forwarded() {
        let state = MidiState::new();
        let targets = NoteLedger::new();
        levels_on(&state);
        assert_eq!(
            decide(&state, &targets, note(5, 72, false), 4),
            Decision::Forward
        );
    }

    #[test]
    fn the_state_starts_without_a_mode() {
        let state = MidiState::new();
        assert_eq!(state.on.load(Ordering::Acquire), 0);
        assert_eq!(state.levels.load(Ordering::Acquire), 0);
        assert_eq!(state.replayed.len(), 128);
        assert_eq!(FREE, 0);
    }
}
