//! Bounded pad press ownership shared by the UI and live input paths.
//!
//! A press publishes its owner before the app receives it.  A release claims
//! the oldest matching press.  Fixed per-pad slots avoid allocation or waiting
//! in a live callback; when full, a new press is refused rather than losing an
//! existing release.

use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use mpc_sample_sdk::Flow;

const PADS: usize = 128;
const PER_PAD: usize = 8;
const FREE: u64 = 0;
const WRITING: u64 = 1;
const EARLY_RELEASE: u64 = 2;
const FORWARDED: u64 = 3;
const CONSUMED: u64 = 4;
const FORCING: u64 = 5;
const FORCING_RELEASED: u64 = 6;
/// A forwarded press whose native handler has not returned yet.
const ARMING: u64 = 7;
/// A forwarded press is reserving its slot.
const WRITING_FORWARDED: u64 = 8;
const STAGE_MASK: u64 = 0xff;

/// The sequence is included in every transition so a stale CAS cannot claim a
/// slot that has since been reused.
fn tagged(sequence: u64, stage: u64) -> u64 {
    (sequence << 8) | stage
}

fn restage(state: u64, phase: u64) -> u64 {
    (state & !STAGE_MASK) | phase
}

struct Slot {
    state: AtomicU64,
    input: AtomicUsize,
    source: AtomicI32,
}

impl Slot {
    const fn new() -> Self {
        Self {
            state: AtomicU64::new(FREE),
            input: AtomicUsize::new(0),
            source: AtomicI32::new(0),
        }
    }
}

/// Fixed-capacity ownership of pad press/release pairs.
pub(crate) struct PadLedger {
    slots: [[Slot; PER_PAD]; PADS],
    /// Active releases take priority over newly arriving presses.
    releases: [AtomicUsize; PADS],
    /// Invalidate a press whose publication overlaps a release or mode close.
    epochs: [AtomicU64; PADS],
    next: AtomicU64,
}

/// Drop the priority gate when the release decision is complete.
struct ReleaseGuard<'a>(&'a AtomicUsize);

impl Drop for ReleaseGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A claimed native release. Complete the claim after sending the note-off.
pub(crate) struct ForcedRelease {
    pub pad: i32,
    pub input: usize,
    pub source: i32,
    index: usize,
    state: u64,
}

impl PadLedger {
    #[allow(
        clippy::large_stack_arrays,
        reason = "The production ledger is static and never allocated in a callback"
    )]
    pub(crate) const fn new() -> Self {
        Self {
            slots: [const { [const { Slot::new() }; PER_PAD] }; PADS],
            releases: [const { AtomicUsize::new(0) }; PADS],
            epochs: [const { AtomicU64::new(0) }; PADS],
            next: AtomicU64::new(0),
        }
    }

    /// Reserve a press. `None` rejects it; a nonzero token needs completion
    /// after the SDK has forwarded the press to the firmware.
    pub(crate) fn press(
        &self,
        pad: usize,
        input: usize,
        source: i32,
        forwarded: bool,
    ) -> Option<u64> {
        let slots = self.slots.get(pad)?;
        let releases = self.releases.get(pad)?;
        let epoch = self.epochs.get(pad)?;
        if releases.load(Ordering::SeqCst) != 0 {
            return None;
        }
        let before = epoch.load(Ordering::SeqCst);
        if releases.load(Ordering::SeqCst) != 0 {
            return None;
        }
        let Ok(sequence) = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                (next < (u64::MAX >> 8)).then_some(next + 1)
            })
        else {
            return None;
        };
        let writing = tagged(
            sequence,
            if forwarded {
                WRITING_FORWARDED
            } else {
                WRITING
            },
        );
        for (index, slot) in slots.iter().enumerate() {
            if slot
                .state
                .compare_exchange(FREE, writing, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            slot.input.store(input, Ordering::Relaxed);
            slot.source.store(source, Ordering::Relaxed);
            let ready = restage(writing, if forwarded { ARMING } else { CONSUMED });
            if slot
                .state
                .compare_exchange(writing, ready, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                if releases.load(Ordering::SeqCst) != 0 || epoch.load(Ordering::SeqCst) != before {
                    // A release may have already changed ARMING to EARLY_RELEASE.
                    // Never clear a slot reused by another press.
                    let _ = slot.state.compare_exchange(
                        ready,
                        FREE,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    let _ = slot.state.compare_exchange(
                        restage(ready, EARLY_RELEASE),
                        FREE,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    return None;
                }
                return Some(if forwarded {
                    u64::try_from(index).ok()? + 1
                } else {
                    0
                });
            }
            // A release arrived while this press was being published: neither
            // edge can be forwarded without creating an unmatched note.
            slot.state.store(FREE, Ordering::Release);
            return None;
        }
        None
    }

    /// Finish a forwarded press. True requests an immediate synthetic release.
    pub(crate) fn finish_press(&self, pad: usize, token: u64, sent: bool) -> bool {
        let Some(index) = token
            .checked_sub(1)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return false;
        };
        let Some(slot) = self.slots.get(pad).and_then(|slots| slots.get(index)) else {
            return false;
        };
        for _ in 0..2 {
            let state = slot.state.load(Ordering::Acquire);
            let phase = state & STAGE_MASK;
            if !matches!(phase, ARMING | EARLY_RELEASE) {
                return false;
            }
            let next = if phase == ARMING && sent {
                restage(state, FORWARDED)
            } else {
                FREE
            };
            if slot
                .state
                .compare_exchange(state, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return phase == EARLY_RELEASE && sent;
            }
        }
        false
    }

    /// Closing a mode must also release presses still entering the firmware.
    pub(crate) fn cancel_forwarding(&self) {
        for (pad, slots) in self.slots.iter().enumerate() {
            self.epochs[pad].fetch_add(1, Ordering::SeqCst);
            for slot in slots {
                let state = slot.state.load(Ordering::Acquire);
                if matches!(state & STAGE_MASK, ARMING | WRITING_FORWARDED) {
                    let _ = slot.state.compare_exchange(
                        state,
                        restage(state, EARLY_RELEASE),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
            }
        }
    }

    /// Claim the oldest matching press, or consume an unmatched release.
    pub(crate) fn release(&self, pad: usize, input: usize, source: i32) -> Flow {
        let Some(slots) = self.slots.get(pad) else {
            return Flow::Consume;
        };
        // Keep new presses out while releases claim existing owners. An epoch
        // catches a press that began before this gate and publishes later.
        self.releases[pad].fetch_add(1, Ordering::SeqCst);
        self.epochs[pad].fetch_add(1, Ordering::SeqCst);
        let _release = ReleaseGuard(&self.releases[pad]);
        // The firmware reports a physical pad's release with a different source
        // id than its press (measured: 0 then 1), so a release first pairs with
        // the same source and then, when none exists, with the same input on
        // this pad. Pairing by source alone left the app's note on forever,
        // which silences the pad after the stale voice is reused.
        for fallback in [false, true] {
            for _ in 0..(PER_PAD * 4) {
                let mut oldest: Option<(usize, u64, u64)> = None;
                for (index, slot) in slots.iter().enumerate() {
                    let state = slot.state.load(Ordering::Acquire);
                    let phase = state & STAGE_MASK;
                    if !matches!(phase, ARMING | FORWARDED | CONSUMED | FORCING)
                        || slot.input.load(Ordering::Relaxed) != input
                        || (!fallback && slot.source.load(Ordering::Relaxed) != source)
                    {
                        continue;
                    }
                    let sequence = state >> 8;
                    if oldest.is_none_or(|(_, _, previous)| sequence < previous) {
                        oldest = Some((index, state, sequence));
                    }
                }
                let Some((index, state, _)) = oldest else {
                    break;
                };
                let phase = state & STAGE_MASK;
                let next = match phase {
                    ARMING => restage(state, EARLY_RELEASE),
                    FORCING => restage(state, FORCING_RELEASED),
                    _ => FREE,
                };
                if slots[index]
                    .state
                    .compare_exchange(state, next, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return if phase == FORWARDED {
                        Flow::Forward
                    } else {
                        Flow::Consume
                    };
                }
            }
        }
        // An unmatched release may overlap a press before its metadata is
        // published. Cancel any such press on this pad rather than forward an
        // unmatched edge. This can reject a concurrent new press.
        for slot in slots {
            let state = slot.state.load(Ordering::Acquire);
            if matches!(state & STAGE_MASK, WRITING | WRITING_FORWARDED) {
                let _ = slot.state.compare_exchange(
                    state,
                    restage(state, EARLY_RELEASE),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
        Flow::Consume
    }

    /// Claim one forwarded press for a synthetic release when closing a mode.
    pub(crate) fn take_forwarded(&self) -> Option<ForcedRelease> {
        for (pad, slots) in self.slots.iter().enumerate() {
            for (index, slot) in slots.iter().enumerate() {
                let state = slot.state.load(Ordering::Acquire);
                if state & STAGE_MASK != FORWARDED {
                    continue;
                }
                let source = slot.source.load(Ordering::Relaxed);
                if slot
                    .state
                    .compare_exchange(
                        state,
                        restage(state, FORCING),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    return Some(ForcedRelease {
                        pad: i32::try_from(pad).ok()?,
                        input: slot.input.load(Ordering::Relaxed),
                        source,
                        index,
                        state: restage(state, FORCING),
                    });
                }
            }
        }
        None
    }

    /// Complete a synthetic note-off, retaining ownership of the physical up.
    pub(crate) fn finish_forced(&self, claim: &ForcedRelease) {
        let Ok(pad) = usize::try_from(claim.pad) else {
            return;
        };
        let Some(slot) = self.slots.get(pad).and_then(|slots| slots.get(claim.index)) else {
            return;
        };
        if slot
            .state
            .compare_exchange(
                claim.state,
                restage(claim.state, CONSUMED),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            let _ = slot.state.compare_exchange(
                restage(claim.state, FORCING_RELEASED),
                FREE,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(ledger: &PadLedger, pad: usize, input: usize, source: i32, forwarded: bool) -> bool {
        let Some(token) = ledger.press(pad, input, source, forwarded) else {
            return false;
        };
        if token != 0 {
            assert!(!ledger.finish_press(pad, token, true));
        }
        true
    }

    #[test]
    fn release_keeps_the_owner_across_mode_changes() {
        let ledger = PadLedger::new();
        assert!(press(&ledger, 3, 10, 1, true));
        assert!(press(&ledger, 3, 20, 1, false));
        assert_eq!(ledger.release(3, 20, 1), Flow::Consume);
        assert_eq!(ledger.release(3, 10, 1), Flow::Forward);
        assert_eq!(ledger.release(3, 10, 1), Flow::Consume);
    }

    #[test]
    fn a_release_with_another_source_still_releases_the_press() {
        let ledger = PadLedger::new();
        assert!(press(&ledger, 4, 9, 0, true));
        // The firmware reports the note-off with source 1 after a source 0 press.
        assert_eq!(ledger.release(4, 9, 1), Flow::Forward);
        assert_eq!(ledger.release(4, 9, 1), Flow::Consume);
    }

    #[test]
    fn an_exact_source_match_wins_over_the_input_fallback() {
        let ledger = PadLedger::new();
        assert!(press(&ledger, 4, 9, 0, true));
        assert!(press(&ledger, 4, 9, 5, true));
        assert_eq!(ledger.release(4, 9, 5), Flow::Forward);
        assert_eq!(ledger.release(4, 9, 1), Flow::Forward);
    }

    #[test]
    fn repeated_presses_release_in_arrival_order_after_slot_reuse() {
        let ledger = PadLedger::new();
        assert!(press(&ledger, 0, 1, 7, true));
        assert!(press(&ledger, 0, 1, 7, false));
        assert_eq!(ledger.release(0, 1, 7), Flow::Forward);
        assert!(press(&ledger, 0, 1, 7, true));
        assert_eq!(ledger.release(0, 1, 7), Flow::Consume);
        assert_eq!(ledger.release(0, 1, 7), Flow::Forward);
    }

    #[test]
    fn a_stale_claim_cannot_take_a_reused_slot() {
        let ledger = PadLedger::new();
        assert!(press(&ledger, 2, 4, 5, true));
        let slot = &ledger.slots[2][0];
        let stale = slot.state.load(Ordering::Acquire);
        assert_eq!(ledger.release(2, 4, 5), Flow::Forward);
        assert!(press(&ledger, 2, 4, 5, true));
        assert!(
            slot.state
                .compare_exchange(stale, FREE, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        );
        assert_eq!(ledger.release(2, 4, 5), Flow::Forward);
    }

    #[test]
    fn exhausted_sequence_rejects_new_presses_instead_of_reusing_a_tag() {
        let ledger = PadLedger::new();
        ledger.next.store(u64::MAX >> 8, Ordering::Relaxed);
        assert!(!press(&ledger, 2, 4, 5, true));
    }

    #[test]
    fn capacity_rejects_new_presses_without_losing_existing_releases() {
        let ledger = PadLedger::new();
        for input in 0..PER_PAD {
            assert!(press(&ledger, 2, input, 5, true));
        }
        assert!(!press(&ledger, 2, 99, 5, true));
        for input in 0..PER_PAD {
            assert_eq!(ledger.release(2, input, 5), Flow::Forward);
        }
        assert_eq!(ledger.release(2, 99, 5), Flow::Consume);
    }

    #[test]
    fn release_overlapping_synthetic_note_off_is_consumed() {
        let ledger = PadLedger::new();
        assert!(press(&ledger, 5, 77, -4, true));
        let claim = ledger.take_forwarded().expect("forwarded press");
        assert_eq!(claim.input, 77);
        assert_eq!(claim.source, -4);
        assert_eq!(ledger.release(5, 77, -4), Flow::Consume);
        ledger.finish_forced(&claim);
        assert!(ledger.take_forwarded().is_none());
        assert!(press(&ledger, 5, 77, -4, true));
        assert_eq!(ledger.release(5, 77, -4), Flow::Forward);
    }

    #[test]
    fn release_before_native_press_returns_gets_a_synthetic_release() {
        let ledger = PadLedger::new();
        let token = ledger.press(1, 2, 3, true).expect("press accepted");
        assert_eq!(ledger.release(1, 2, 3), Flow::Consume);
        assert!(ledger.finish_press(1, token, true));
        assert_eq!(ledger.release(1, 2, 3), Flow::Consume);
    }

    #[test]
    fn closing_during_native_press_releases_after_forwarding() {
        let ledger = PadLedger::new();
        let token = ledger.press(1, 2, 3, true).expect("press accepted");
        ledger.cancel_forwarding();
        assert!(ledger.finish_press(1, token, true));
    }

    #[test]
    fn closing_cancels_a_forwarded_press_during_publication() {
        let ledger = PadLedger::new();
        let slot = &ledger.slots[1][0];
        slot.state
            .store(tagged(42, WRITING_FORWARDED), Ordering::Release);
        ledger.cancel_forwarding();
        assert_eq!(
            slot.state.load(Ordering::Acquire),
            tagged(42, EARLY_RELEASE)
        );
    }

    #[test]
    fn release_overlapping_publication_cancels_the_press() {
        let ledger = PadLedger::new();
        let slot = &ledger.slots[1][0];
        slot.state.store(tagged(42, WRITING), Ordering::Release);
        assert_eq!(ledger.release(1, 123, 42), Flow::Consume);
        assert_eq!(
            slot.state.load(Ordering::Acquire),
            tagged(42, EARLY_RELEASE)
        );
    }
}
