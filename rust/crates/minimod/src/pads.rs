//! Pad occupancy as the app's own pad finder sees it.
//!
//! Extract needs a free pad, and the mod has to know how many there are before
//! and after it runs an extraction: the difference is the only reliable signal
//! that the app actually assigned the sample.

use crate::firmware;

/// The app's "count free pads" entry, as an offset into the program object's
/// vtable call.
const FREE_PAD: usize = 0x286_dd00;
/// The app's "is this pad state busy" entry.
const PAD_BUSY: usize = 0x280_47b0;
/// `[program + 0x2808]` is the first pad state, `+0x2810` the last.
const PAD_STATES: usize = 0x2808;
/// The app never has more pads than this.
const MAX_PADS: usize = 128;

/// The app's count of free pads.
#[must_use]
pub fn free_count(base: usize, program: usize) -> i32 {
    // SAFETY: `base + FREE_PAD` is the entry the app itself calls with its
    // program object.
    unsafe { firmware::int_of_pointer(base + FREE_PAD, program) }
}

/// How many pads the app's pad states say are free.
///
/// The same two checks the native finder uses: a null pad state, or a pad state
/// the app reports as not busy.
#[must_use]
pub fn free_estimate(base: usize, program: usize) -> usize {
    let Some(first) = firmware::read_pointer(program + PAD_STATES) else {
        return 0;
    };
    let Some(last) = firmware::read_pointer(program + PAD_STATES + 8) else {
        return 0;
    };
    if first == 0 || last < first {
        return 0;
    }
    let count = ((last - first) / 8).min(MAX_PADS);
    let mut free = 0;
    for index in 0..count {
        let Some(state) = firmware::read_pointer(first + index * 8) else {
            break;
        };
        if state == 0 {
            free += 1;
            continue;
        }
        // SAFETY: `base + PAD_BUSY` is the app's own predicate for a pad state.
        let busy = unsafe { firmware::int_of_pointer(base + PAD_BUSY, state) };
        if busy == 0 {
            free += 1;
        }
    }
    free
}

/// The app's count of free pads.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_pads_free_pad(base: usize, program: usize) -> i32 {
    free_count(base, program)
}

/// The pad states' count of free pads.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_pads_free_estimate(base: usize, program: usize) -> i32 {
    i32::try_from(free_estimate(base, program)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{FREE_PAD, MAX_PADS, PAD_BUSY, PAD_STATES};

    #[test]
    fn the_offsets_are_the_documented_ones() {
        assert_eq!(FREE_PAD, 0x286_dd00);
        assert_eq!(PAD_BUSY, 0x280_47b0);
        assert_eq!(PAD_STATES, 0x2808);
        assert_eq!(MAX_PADS, 128);
    }
}
