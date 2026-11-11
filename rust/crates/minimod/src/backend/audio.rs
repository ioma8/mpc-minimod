//! The mod's audio work: feeding the looper, mixing it back, and auditioning the
//! bass synth.
//!
//! The SDK owns the ALSA taps and the stream geometry; this module only sees the
//! S32 frames the app is about to play or has just captured.

use core::sync::atomic::{AtomicBool, Ordering};

use mpc_sample_sdk::audio::OutputInfo;

/// Whether the previous block carried the preview, so the voices are cleared
/// once when it stops.
static WAS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The output tap: mix the preview and the loops into the block about to play.
pub fn output(frames: &mut [i32], info: &OutputInfo) {
    if frames.is_empty() || info.channels == 0 || info.block_frames == 0 {
        return;
    }
    mix_preview(frames, info);
    if crate::backend::looper::looper_is_open() != 0 {
        crate::backend::looper::looper_render_s32(frames, info.channels);
        measure_latency(info);
    }
}

/// Planar input: hand the captured channels to the looper.
pub fn input_planar(channels: &[&[i32]], _rate: u32) {
    if crate::backend::looper::looper_is_open() == 0 || channels.is_empty() {
        return;
    }
    crate::backend::looper::looper_input_s32(channels);
}

/// Interleaved input: hand the block to the looper.
pub fn input_interleaved(frames: &[i32], channels: usize, _rate: u32) {
    if crate::backend::looper::looper_is_open() == 0 || channels == 0 || frames.is_empty() {
        return;
    }
    crate::backend::looper::looper_input_s32_interleaved(frames, channels);
}

/// Frames mixed per preview call: bounds the stack scratch, so the audio thread
/// never allocates.
const PREVIEW_CHUNK: usize = 512;

/// Add the bass preview to the interleaved block.
///
/// The preview is an audition, not a saved sample; its voices are owned by the
/// DSP's own singleton and only the flag is read here.
fn mix_preview(frames: &mut [i32], info: &OutputInfo) {
    let active = crate::backend::bass::preview_active();
    let was = WAS_ACTIVE.swap(active, Ordering::AcqRel);
    if !active {
        if was {
            // An empty list mixes nothing and clears the voices.
            // SAFETY: this is the single output audio callback, so it is the
            // only caller of the process-wide preview mixer.
            unsafe {
                minimod_dsp::bass::preview_mix_slices(&mut [], 0, info.rate);
            }
        }
        return;
    }
    let channels = info.channels;
    let count = channels.min(2);
    if !(1..=8).contains(&channels) || info.block_frames * channels > frames.len() {
        return;
    }
    let mut planes = [0.0_f32; PREVIEW_CHUNK * 2];
    let mut done = 0;
    while done < info.block_frames {
        let length = (info.block_frames - done).min(PREVIEW_CHUNK);
        let (left, right) = planes.split_at_mut(PREVIEW_CHUNK);
        let left = &mut left[..length];
        let right = &mut right[..length];
        left.fill(0.0);
        right.fill(0.0);
        let mut slices: [&mut [f32]; 2] = [left, right];
        // SAFETY: this is the single output audio callback, the only caller of
        // the preview mixer; each slice holds `length` writable samples.
        let _ = unsafe {
            minimod_dsp::bass::preview_mix_slices(&mut slices[..count], length, info.rate)
        };
        let planes: [&[f32]; 2] = [slices[0], slices[1]];
        let start = done * channels;
        let end = (done + length) * channels;
        let _ = mpc_sample_sdk::audio::add_planes_i32(
            &mut frames[start..end],
            channels,
            &planes[..count],
        );
        done += length;
    }
}

/// Hand the looper the device's measured latency.
fn measure_latency(info: &OutputInfo) {
    let backlog = crate::backend::looper::looper_input_backlog();
    let (total, source) = info.latency(backlog, mpc_sample_sdk::audio::extra_latency());
    crate::backend::looper::looper_set_latency_ex(total, i8::try_from(source).unwrap_or(0x45));
}
