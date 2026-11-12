//! The looper's persisted settings.
//!
//! The MAIN VOLUME survives a restart. The file is deliberately trivial -
//! `"<master>"` - but it is read at startup and rewritten whenever it changes,
//! so a malformed or half-written file must never leave the looper in a strange
//! state.

use std::io::Write;
use std::path::Path;

/// Where the settings live.
pub const PATH: &str = "/data/minimod-looper.cfg";
/// What the file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// MAIN VOLUME when the file carried one.
    pub master: Option<u32>,
}

/// Read the settings, or `None` when the file is absent or unusable.
#[must_use]
pub fn load(path: &Path) -> Option<Settings> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut fields = text.split_whitespace();
    let first: i32 = fields.next()?.parse().ok()?;
    // Old files wrote "<trim_ms> <master>"; new files write only "<master>".
    let master = match fields.next() {
        Some(field) => field.parse::<i32>().ok()?,
        None => first,
    };
    Some(Settings {
        master: Some(u32::try_from(master.clamp(0, 127)).unwrap_or(127)),
    })
}

/// Write the settings atomically, replacing the file.
///
/// # Errors
///
/// Returns the standard I/O error of the temporary create, write or rename.
pub fn save(path: &Path, settings: Settings) -> std::io::Result<()> {
    let master = settings.master.unwrap_or(127).min(127);
    let text = format!("{master}\n");
    // Write a sibling and rename it over the real file, so a crash or a full
    // disk leaves the previous settings intact instead of a truncated line.
    let temporary = path.with_extension("tmp");
    let written =
        std::fs::File::create(&temporary).and_then(|mut file| file.write_all(text.as_bytes()));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    // No `sync_data`: this runs on the settings thread, and losing the last
    // trim or volume on a power cut is a better trade than a file-system stall.
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Settings, load, save};

    fn path(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("minimod-cfg-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp dir");
        directory.join("settings.cfg")
    }

    #[test]
    fn a_saved_file_round_trips() {
        let path = path("round-trip");
        save(&path, Settings { master: Some(64) }).expect("save");
        assert_eq!(load(&path), Some(Settings { master: Some(64) }));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn an_old_trim_and_master_file_keeps_only_the_master() {
        let path = path("old");
        std::fs::write(&path, "12 64\n").expect("write");
        assert_eq!(load(&path), Some(Settings { master: Some(64) }));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn the_master_is_clamped_on_the_way_in() {
        let path = path("clamp");
        std::fs::write(&path, "9999\n").expect("write");
        assert_eq!(load(&path), Some(Settings { master: Some(127) }));
        std::fs::write(&path, "-5\n").expect("write");
        assert_eq!(load(&path), Some(Settings { master: Some(0) }));
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
