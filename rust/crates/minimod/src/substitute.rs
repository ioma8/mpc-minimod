//! Feeding the app a rendered buffer instead of its own sample.
//!
//! The effects and the bass synth do not hand the app a file: they render into
//! memory and intercept the writer the app calls while saving, replacing each
//! block's pointer with a window into the rendered buffer.  The tail block is
//! the interesting one - when the rendered audio is longer than the source (a
//! reverb tail), the last block is enlarged so the extra audio is written too.

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Where the rendered buffer is and how to walk it.
#[derive(Clone, Copy, Debug)]
struct Plan {
    buffer: usize,
    /// Frames the app will ask for (the source's length).
    source_frames: usize,
    /// Frames actually available (source plus tail).
    frames: usize,
    channels: usize,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static BUFFER: AtomicUsize = AtomicUsize::new(0);
static SOURCE_FRAMES: AtomicUsize = AtomicUsize::new(0);
static FRAMES: AtomicUsize = AtomicUsize::new(0);
static CHANNELS: AtomicUsize = AtomicUsize::new(0);
static POSITION: AtomicUsize = AtomicUsize::new(0);

/// Start substituting `buffer` (interleaved, `channels` wide) for the app's
/// sample of `source_frames`, with `frames` of audio available in total.
pub fn arm(buffer: usize, frames: usize, source_frames: usize, channels: usize) {
    BUFFER.store(buffer, Ordering::Release);
    FRAMES.store(frames, Ordering::Release);
    SOURCE_FRAMES.store(source_frames, Ordering::Release);
    CHANNELS.store(channels, Ordering::Release);
    POSITION.store(0, Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
}

/// Stop substituting.
pub fn clear() {
    ACTIVE.store(false, Ordering::Release);
}

/// Whether a substitution is running.
#[must_use]
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// The plan, or `None` when nothing is armed.
fn plan() -> Option<Plan> {
    if !active() {
        return None;
    }
    let plan = Plan {
        buffer: BUFFER.load(Ordering::Acquire),
        frames: FRAMES.load(Ordering::Acquire),
        source_frames: SOURCE_FRAMES.load(Ordering::Acquire),
        channels: CHANNELS.load(Ordering::Acquire),
    };
    (plan.buffer != 0 && plan.channels != 0).then_some(plan)
}

/// The app's writer: either forward the block, or point it at our buffer.
///
/// # Safety
///
/// `block` must be the app's block descriptor (`u32` frames at `+0`, a sample
/// pointer at `+8`) and `writer` the original writer for the same closure.
pub unsafe fn write(closure: usize, block: *mut u8, writer: Option<Writer>) {
    if block.is_null() {
        forward(closure, block, writer);
        return;
    }
    let Some(plan) = plan() else {
        forward(closure, block, writer);
        return;
    };
    // SAFETY: the app's block descriptor is a frame count followed by a pointer.
    let frames = unsafe { core::ptr::read_unaligned(block.cast::<u32>()) };
    let pointer = unsafe { core::ptr::read_unaligned(block.add(8).cast::<usize>()) };
    let position = POSITION.load(Ordering::Acquire);
    let frames = usize::try_from(frames).unwrap_or(0);
    if position + frames > plan.source_frames || frames == 0 {
        forward(closure, block, writer);
        return;
    }
    let replacement = plan.buffer + position * plan.channels * size_of::<f32>();
    let tail = position + frames == plan.source_frames && plan.frames > plan.source_frames;
    let write_frames = if tail { plan.frames - position } else { frames };
    if tail {
        let Ok(extended) = u32::try_from(write_frames) else {
            forward(closure, block, writer);
            return;
        };
        // SAFETY: see above; the block's frame count is writable.
        unsafe { core::ptr::write_unaligned(block.cast::<u32>(), extended) };
    }
    // SAFETY: see above; the pointer slot is writable.
    unsafe { core::ptr::write_unaligned(block.add(8).cast::<usize>(), replacement) };
    forward(closure, block, writer);
    // Put the app's own descriptor back: it owns both fields.
    unsafe {
        if tail {
            core::ptr::write_unaligned(block.cast::<u32>(), u32::try_from(frames).unwrap_or(0));
        }
        core::ptr::write_unaligned(block.add(8).cast::<usize>(), pointer);
    }
    POSITION.store(
        if tail { plan.frames } else { position + frames },
        Ordering::Release,
    );
}

/// Forward the block to the app's own writer.
fn forward(closure: usize, block: *mut u8, writer: Option<Writer>) {
    if let Some(writer) = writer {
        // SAFETY: the caller guarantees the signature matches the app's writer.
        unsafe { writer(closure, block as usize) };
    }
}

/// The app's writer signature: closure plus the block descriptor.
pub type Writer = unsafe extern "C" fn(usize, usize);

/// The app's writer entry, for the C glue.
///
/// # Safety
///
/// `block` must be the app's block descriptor and `writer` its original writer.
pub unsafe extern "C" fn minimod_substitute(
    closure: usize,
    block: *mut u8,
    writer: Option<Writer>,
) {
    // SAFETY: see the function contract.
    unsafe { write(closure, block, writer) };
}

/// How far the writer has consumed the rendered buffer.
#[must_use]
pub fn position() -> usize {
    POSITION.load(Ordering::Acquire)
}

/// The number of frames the writer has to consume in total.
#[must_use]
pub fn total_frames() -> usize {
    FRAMES.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::{arm, clear, plan};

    #[test]
    fn arming_and_clearing_flips_the_plan() {
        clear();
        assert!(plan().is_none());
        let buffer = [0.0_f32; 8];
        arm(buffer.as_ptr() as usize, 8, 8, 1);
        let armed = plan().expect("armed");
        assert_eq!(armed.frames, 8);
        assert_eq!(armed.source_frames, 8);
        assert_eq!(armed.channels, 1);
        clear();
        assert!(plan().is_none());
    }

    #[test]
    fn a_zero_buffer_is_not_armed() {
        arm(0, 8, 8, 1);
        assert!(plan().is_none());
        clear();
    }
}
