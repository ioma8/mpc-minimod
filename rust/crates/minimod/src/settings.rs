//! The looper's persisted settings, as safe values.

use std::path::Path;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender};

use crate::cfg;

/// One write handed to the settings thread.
#[derive(Clone, Copy)]
struct Request {
    /// MAIN VOLUME.
    master: u32,
}

/// The settings writer's queue, created on the first save.
///
/// `save` runs from the CC callback, which holds the module lock the SDK's audio
/// dispatch uses; a file write there can cost an audio block.  The write itself
/// is therefore handed to one long-lived thread.
fn writer() -> Option<&'static Sender<Request>> {
    static WRITER: OnceLock<Option<Sender<Request>>> = OnceLock::new();
    WRITER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Request>();
            std::thread::Builder::new()
                .name("minimod-settings".to_string())
                .spawn(move || {
                    write_requests(&receiver, Path::new(cfg::PATH));
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

/// Persist the latest queued value as soon as possible on the writer thread.
fn write_requests(receiver: &Receiver<Request>, path: &Path) {
    while let Ok(mut request) = receiver.recv() {
        for next in receiver.try_iter() {
            request = next;
        }
        let _ = cfg::save(
            path,
            cfg::Settings {
                master: Some(request.master),
            },
        );
    }
}

/// Read the settings file, or `None` when it is absent or unusable.
#[must_use]
pub fn load() -> Option<cfg::Settings> {
    cfg::load(Path::new(cfg::PATH))
}

/// Queue the settings file write.
///
/// The value is written by the settings thread; if the thread could not be
/// started (or has stopped), the write happens on the caller instead of being
/// lost.
pub fn save(master: u32) {
    let request = Request { master };
    if writer().is_some_and(|sender| sender.send(request).is_ok()) {
        return;
    }
    let _ = cfg::save(
        Path::new(cfg::PATH),
        cfg::Settings {
            master: Some(master),
        },
    );
}

/// Load the persisted settings into the running mod.
pub fn load_into_mod() {
    let Some(settings) = load() else {
        return;
    };
    if let Some(master) = settings.master {
        crate::backend::looper::looper_set_master(master);
    }
}

#[cfg(test)]
mod tests {
    use super::{Request, write_requests};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn settings_are_written_without_waiting_for_the_sender_to_close() {
        let directory =
            std::env::temp_dir().join(format!("minimod-settings-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.cfg");
        let (sender, receiver) = mpsc::channel();
        let target = path.clone();
        let writer = std::thread::spawn(move || write_requests(&receiver, &target));
        sender.send(Request { master: 43 }).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while crate::cfg::load(&path).and_then(|value| value.master) != Some(43) {
            assert!(
                std::time::Instant::now() < deadline,
                "writer waited for more input"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(sender);
        writer.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
