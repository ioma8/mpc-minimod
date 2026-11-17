//! Recent files for the stock Load Sample browser.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, OsStr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

const FOLDER: &str = "Recent Samples";
const MAX_RECENT: usize = 20;

static RECENT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static DATA_PRESSED: AtomicBool = AtomicBool::new(false);

/// Install the native browser entry and observe successful source-file opens.
pub(crate) fn start() {
    set_directories(&[]);
    mpc_sample_sdk::browser::set_file_open_observer(Some(file_opened));
}

pub(crate) fn on_button(event: mpc_sample_sdk::ButtonEvent) {
    if event.code == mpc_sample_sdk::event::button::DATA {
        DATA_PRESSED.store(event.pressed, Ordering::Release);
    }
}

fn set_directories(entries: &[mpc_sample_sdk::browser::VirtualFile]) {
    if let Ok(mounts) = std::fs::read_dir("/media") {
        for mount in mounts
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
        {
            let _ =
                mpc_sample_sdk::browser::set_virtual_directory(&mount, OsStr::new(FOLDER), entries);
        }
    }
}

unsafe extern "C" fn file_opened(path: *const std::ffi::c_char) {
    let data_pressed = DATA_PRESSED.swap(false, Ordering::AcqRel);
    if !data_pressed {
        return;
    }
    if path.is_null() {
        return;
    }
    // SAFETY: the SDK passes a live, NUL-terminated path for this callback.
    let path = unsafe { CStr::from_ptr(path) };
    let Some(path) = path.to_str().ok().map(PathBuf::from) else {
        return;
    };
    if !should_track_open(data_pressed, &path) {
        return;
    }

    let mut recent = lock(&RECENT);
    recent.retain(|existing| existing != &path);
    recent.insert(0, path);
    recent.truncate(MAX_RECENT);
    let entries = virtual_files(&recent);
    set_directories(&entries);
}

fn should_track_open(data_pressed: bool, path: &Path) -> bool {
    data_pressed && is_sample(path)
}

fn virtual_files(paths: &[PathBuf]) -> Vec<mpc_sample_sdk::browser::VirtualFile> {
    let mut counts = HashMap::<&str, usize>::new();
    for path in paths {
        if let Some(name) = path.file_name().and_then(OsStr::to_str) {
            *counts.entry(name).or_default() += 1;
        }
    }

    let mut labels = HashSet::new();
    paths
        .iter()
        .filter_map(|path| {
            let file_name = path.file_name()?.to_str()?;
            let label = if counts.get(file_name).copied().unwrap_or_default() > 1 {
                let stem = path.file_stem()?.to_str()?;
                let extension = path.extension()?.to_str()?;
                let parent = path.parent()?.file_name()?.to_str()?;
                let base = format!("{stem} — {parent}");
                let mut label = format!("{base}.{extension}");
                let mut suffix = 2;
                while labels.contains(&label) {
                    label = format!("{base} {suffix}.{extension}");
                    suffix += 1;
                }
                label
            } else {
                file_name.to_owned()
            };
            if !labels.insert(label.clone()) {
                return None;
            }
            mpc_sample_sdk::browser::VirtualFile::new(OsStr::new(&label), path.clone())
        })
        .collect()
}

fn is_sample(path: &Path) -> bool {
    let in_samples = media_root(path).is_some() || path.starts_with("/data/UserContent/Samples");
    let audio = path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "wav" | "aif" | "aiff" | "mp3" | "flac" | "ogg"
        )
    });
    in_samples && audio
}

fn media_root(path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix("/media").ok()?;
    let mut components = relative.components();
    let mount = components.next()?.as_os_str();
    components.next()?;
    Some(Path::new("/media").join(mount))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{is_sample, media_root, should_track_open, virtual_files};
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    #[test]
    fn only_sample_audio_files_are_recent() {
        assert!(is_sample(Path::new("/media/OTHER CARD/Kits/808/Kick.WAV")));
        assert!(is_sample(Path::new(
            "/media/MPC SAMPLE/MPC-Sample/Samples/Kits/808/Kick.WAV"
        )));
        assert!(is_sample(Path::new(
            "/data/UserContent/Samples/User/voice.aiff"
        )));
        assert!(!is_sample(Path::new(
            "/data/TempProjects/Temp-Project_[ProjectData]/sample.wav"
        )));
        assert!(!is_sample(Path::new(
            "/media/MPC SAMPLE/MPC-Sample/Projects/demo.xpj"
        )));
    }

    #[test]
    fn mount_root_uses_the_actual_media_label() {
        assert_eq!(
            media_root(Path::new("/media/OTHER CARD/Kits/kick.wav")),
            Some(PathBuf::from("/media/OTHER CARD"))
        );
        assert_eq!(media_root(Path::new("/media/kick.wav")), None);
    }

    #[test]
    fn preview_opens_are_not_recent_until_data_loads_the_sample() {
        let path = Path::new("/media/OTHER CARD/Kits/808/Kick.WAV");
        assert!(!should_track_open(false, path));
        assert!(should_track_open(true, path));
        assert!(!should_track_open(
            true,
            Path::new("/media/OTHER CARD/Projects/demo.xpj")
        ));
    }

    #[test]
    fn duplicate_names_stay_distinguishable_and_keep_their_sources() {
        let files = virtual_files(&[
            PathBuf::from("/media/MPC SAMPLE/MPC-Sample/Samples/Kits/A/kick.wav"),
            PathBuf::from("/media/MPC SAMPLE/MPC-Sample/Samples/Kits/B/kick.wav"),
        ]);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name(), OsStr::new("kick — A.wav"));
        assert_eq!(files[1].name(), OsStr::new("kick — B.wav"));
        assert_eq!(
            files[0].source(),
            Path::new("/media/MPC SAMPLE/MPC-Sample/Samples/Kits/A/kick.wav")
        );
    }
}
