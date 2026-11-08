//! The looper's persisted settings.
//!
//! Two numbers survive a restart: the latency trim in milliseconds and the
//! MAIN VOLUME.  The file is deliberately trivial - `"<trim_ms> <master>"` - but
//! it is read at startup and rewritten whenever either changes, so a malformed
//! or half-written file must never leave the looper in a strange state.

use std::io::Write;
use std::path::Path;

/// Where the settings live.
pub const PATH: &str = "/data/minimod-looper.cfg";
/// Frames per millisecond at the device's 44.1 kHz.
pub const FRAMES_PER_MS: i32 = 44;
/// The trim is bounded to this many milliseconds either way.
pub const TRIM_LIMIT_MS: i32 = 400;

/// What the file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Latency trim in frames.
    pub trim_frames: i32,
    /// MAIN VOLUME when the file carried one.
    pub master: Option<u32>,
}

/// Read the settings, or `None` when the file is absent or unusable.
#[must_use]
pub fn load(path: &Path) -> Option<Settings> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut fields = text.split_whitespace();
    let trim_ms: i32 = fields.next()?.parse().ok()?;
    let master = fields
        .next()
        .and_then(|field| field.parse::<i32>().ok())
        .map(|value| u32::try_from(value.clamp(0, 127)).unwrap_or(127));
    Some(Settings {
        trim_frames: trim_ms.clamp(-TRIM_LIMIT_MS, TRIM_LIMIT_MS) * FRAMES_PER_MS,
        master,
    })
}

/// Write the settings, replacing the file.
///
/// # Errors
///
/// Returns the standard I/O error of the open, write or flush.
pub fn save(path: &Path, settings: Settings) -> std::io::Result<()> {
    let master = settings.master.unwrap_or(127).min(127);
    let text = format!("{} {master}\n", settings.trim_frames / FRAMES_PER_MS);
    let mut file = std::fs::File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_data()
}

#[cfg(test)]
mod tests {
    use super::{FRAMES_PER_MS, Settings, load, save};

    fn path(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("minimod-cfg-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp dir");
        directory.join("settings.cfg")
    }

    #[test]
    fn a_saved_file_round_trips() {
        let path = path("round-trip");
        save(
            &path,
            Settings {
                trim_frames: -176,
                master: Some(64),
            },
        )
        .expect("save");
        assert_eq!(
            load(&path),
            Some(Settings {
                trim_frames: -176,
                master: Some(64)
            })
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn the_values_are_clamped_on_the_way_in() {
        let path = path("clamp");
        std::fs::write(&path, "99999 9999\n").expect("write");
        assert_eq!(
            load(&path),
            Some(Settings {
                trim_frames: 400 * FRAMES_PER_MS,
                master: Some(127)
            })
        );
        std::fs::write(&path, "-99999 -5\n").expect("write");
        assert_eq!(
            load(&path),
            Some(Settings {
                trim_frames: -400 * FRAMES_PER_MS,
                master: Some(0)
            })
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_trim_without_a_volume_is_accepted() {
        let path = path("partial");
        std::fs::write(&path, "12\n").expect("write");
        assert_eq!(
            load(&path),
            Some(Settings {
                trim_frames: 12 * FRAMES_PER_MS,
                master: None
            })
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn junk_and_missing_files_are_refused() {
        let path = path("junk");
        std::fs::write(&path, "not a number\n").expect("write");
        assert_eq!(load(&path), None);
        std::fs::write(&path, "").expect("write");
        assert_eq!(load(&path), None);
        assert_eq!(load(&path.with_extension("absent")), None);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
