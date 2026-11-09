//! Finding the audible bounds of a recording.
//!
//! The trim page trims a recording to the first and last frame above a
//! threshold.  The threshold is relative to the file's own peak - so quiet
//! material is trimmed as well - but never below ~-51 dBFS, so a nearly silent
//! file is not trimmed to nothing.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::wav::{self, Encoding};

/// Bytes read per block; one read per frame froze the UI thread on long samples.
const BLOCK_BYTES: usize = 16_384;
/// Threshold used when the caller does not override it: 3 % of the peak.
const DEFAULT_DB: f64 = -30.457_574_905_606_75;
/// Floor of the relative threshold (~-51 dBFS).
const MIN_THRESHOLD: f64 = 90.0;
/// Ceiling of the relative threshold.
const MAX_THRESHOLD: f64 = 30_000.0;

/// Where the sample data lives and how to read it.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// File offset of the first sample.
    pub data_offset: u64,
    /// Interleaved channels.
    pub channels: u16,
    /// Bytes per sample of one channel.
    pub width: u16,
    /// Integer or float samples.
    pub encoding: Encoding,
}

impl Layout {
    /// Bytes of one interleaved frame.
    fn frame_bytes(&self) -> usize {
        usize::from(self.width) * usize::from(self.channels)
    }
}

/// The first and last audible frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    /// First frame above the threshold.
    pub first: u32,
    /// Last frame above the threshold.
    pub last: u32,
}

/// Find the audible bounds of `path`, scanning `frames` interleaved frames.
///
/// `ratio` overrides the relative threshold (0.0 means "use the default, or
/// `MINIMOD_TRIM_DB` when it is set").
#[must_use]
pub fn edges(path: &Path, layout: Layout, frames: u32, ratio: f64) -> Option<Edges> {
    let channels = usize::from(layout.channels.max(1));
    let mut layout = layout;
    layout.channels = u16::try_from(channels).unwrap_or(1);
    if !(2..=4).contains(&layout.width) {
        layout.width = 2;
    }
    let frame_bytes = layout.frame_bytes();
    if frame_bytes == 0 {
        return None;
    }
    let file = File::open(path).ok()?;
    let threshold = threshold(&file, &layout, frames, frame_bytes, ratio);
    let first = first_loud(&file, &layout, frames, frame_bytes, threshold)?;
    let last = last_loud(&file, &layout, frames, frame_bytes, threshold, first);
    Some(Edges { first, last })
}

/// The amplitude a frame must exceed to count as audible.
fn threshold(file: &File, layout: &Layout, frames: u32, frame_bytes: usize, ratio: f64) -> i32 {
    let ratio = if ratio > 0.0 {
        ratio
    } else {
        let db = std::env::var("MINIMOD_TRIM_DB")
            .ok()
            .and_then(|text| text.trim().parse::<f64>().ok())
            .unwrap_or(DEFAULT_DB)
            .clamp(-90.0, -6.0);
        10.0_f64.powf(db / 20.0)
    };
    let peak = peak(file, layout, frames, frame_bytes);
    if peak == 0 {
        return 328;
    }
    let scaled = (f64::from(peak) * ratio).clamp(MIN_THRESHOLD, MAX_THRESHOLD);
    scaled as i32
}

/// The loudest sample in the file, in the trim scale.
fn peak(file: &File, layout: &Layout, frames: u32, frame_bytes: usize) -> i32 {
    let mut buffer = [0_u8; BLOCK_BYTES];
    let per_block = BLOCK_BYTES / frame_bytes;
    let mut peak = 0;
    let mut at = 0_u32;
    while at < frames {
        let Some(read) = read_frames(file, layout, at, per_block, &mut buffer) else {
            break;
        };
        for frame in 0..read {
            for channel in 0..usize::from(layout.channels) {
                let offset =
                    (frame * usize::from(layout.channels) + channel) * usize::from(layout.width);
                let value = wav::scaled_sample(
                    &buffer[offset..offset + usize::from(layout.width)],
                    layout.width,
                    layout.encoding,
                )
                .abs();
                peak = peak.max(value);
            }
        }
        at += u32::try_from(read).unwrap_or(0);
        if read < per_block {
            break;
        }
    }
    peak
}

/// The first frame that is louder than `threshold`.
fn first_loud(
    file: &File,
    layout: &Layout,
    frames: u32,
    frame_bytes: usize,
    threshold: i32,
) -> Option<u32> {
    let mut buffer = [0_u8; BLOCK_BYTES];
    let per_block = BLOCK_BYTES / frame_bytes;
    let mut at = 0_u32;
    while at < frames {
        let read = read_frames(file, layout, at, per_block, &mut buffer)?;
        for frame in 0..read {
            if loud(layout, &buffer, frame, threshold) {
                return Some(at + u32::try_from(frame).unwrap_or(0));
            }
        }
        at += u32::try_from(read).unwrap_or(0);
        if read < per_block {
            break;
        }
    }
    None
}

/// The last frame that is louder than `threshold`, never before `first`.
fn last_loud(
    file: &File,
    layout: &Layout,
    frames: u32,
    frame_bytes: usize,
    threshold: i32,
    first: u32,
) -> u32 {
    let mut buffer = [0_u8; BLOCK_BYTES];
    let per_block = BLOCK_BYTES / frame_bytes;
    let last = first;
    let mut end = frames;
    while end > 0 {
        let count = per_block.min(usize::try_from(end).unwrap_or(0));
        let start = end - u32::try_from(count).unwrap_or(0);
        let written = file.read_exact_at(
            &mut buffer[..count * frame_bytes],
            layout.data_offset + u64::from(start) * u64::try_from(frame_bytes).unwrap_or(0),
        );
        if written.is_err() {
            break;
        }
        for frame in (0..count).rev() {
            if loud(layout, &buffer, frame, threshold) {
                return start + u32::try_from(frame).unwrap_or(0);
            }
        }
        end = start;
    }
    last
}

/// Read up to `frames` interleaved frames at `at`.
fn read_frames(
    file: &File,
    layout: &Layout,
    at: u32,
    frames: usize,
    buffer: &mut [u8],
) -> Option<usize> {
    let frame_bytes = layout.frame_bytes();
    let want = frames * frame_bytes;
    let offset = layout.data_offset + u64::from(at) * u64::try_from(frame_bytes).ok()?;
    let read = file.read_at(&mut buffer[..want], offset).ok()?;
    Some(read / frame_bytes)
}

/// Whether any channel of `frame` is louder than `threshold`.
fn loud(layout: &Layout, buffer: &[u8], frame: usize, threshold: i32) -> bool {
    let width = usize::from(layout.width);
    let channels = usize::from(layout.channels);
    (0..channels).any(|channel| {
        let offset = (frame * channels + channel) * width;
        wav::scaled_sample(
            &buffer[offset..offset + width],
            layout.width,
            layout.encoding,
        )
        .abs()
            > threshold
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Edges, Layout, edges};
    use crate::wav::Info;

    /// Write a 16-bit mono WAV whose `frames` are `loud` inside `first..last`.
    fn padded(name: &str, frames: u32, first: u32, last: u32) -> (PathBuf, Layout) {
        let mut payload = Vec::new();
        for frame in 0..frames {
            let value: i16 = if frame >= first && frame <= last {
                12_000
            } else {
                0
            };
            payload.extend_from_slice(&value.to_le_bytes());
        }
        let directory = std::env::temp_dir().join(format!("minimod-trim-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("take.wav");
        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&16_u32.to_le_bytes());
        body.extend_from_slice(&1_u16.to_le_bytes());
        body.extend_from_slice(&1_u16.to_le_bytes());
        body.extend_from_slice(&44_100_u32.to_le_bytes());
        body.extend_from_slice(&88_200_u32.to_le_bytes());
        body.extend_from_slice(&2_u16.to_le_bytes());
        body.extend_from_slice(&16_u16.to_le_bytes());
        body.extend_from_slice(b"data");
        body.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(0).to_le_bytes());
        body.extend_from_slice(&payload);
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&u32::try_from(body.len() + 4).unwrap_or(0).to_le_bytes());
        file.extend_from_slice(b"WAVE");
        file.extend_from_slice(&body);
        std::fs::write(&path, &file).expect("write");
        let info = Info::read(&path).expect("info");
        (path, layout_of(&info))
    }

    fn layout_of(info: &Info) -> Layout {
        Layout {
            data_offset: info.data_offset,
            channels: info.channels,
            width: info.width,
            encoding: info.encoding,
        }
    }

    #[test]
    fn finds_the_padded_audio() {
        let (path, layout) = padded("bounds", 1_000, 100, 800);
        assert_eq!(
            edges(&path, layout, 1_000, 0.03),
            Some(Edges {
                first: 100,
                last: 800
            })
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_silent_file_has_no_edges() {
        let (path, layout) = padded("silent", 500, 1, 0);
        assert_eq!(edges(&path, layout, 500, 0.03), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_higher_ratio_trims_tighter() {
        let (path, layout) = padded("ratio", 1_000, 200, 700);
        // A ratio above 1 makes the threshold higher than the peak, so nothing
        // is loud enough.
        assert_eq!(edges(&path, layout, 1_000, 2.0), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
