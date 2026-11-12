//! The looper's persisted settings, as safe values.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

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
                    while let Ok(request) = receiver.recv() {
                        let _ = cfg::save(
                            std::path::Path::new(cfg::PATH),
                            cfg::Settings {
                                master: Some(request.master),
                            },
                        );
                    }
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

/// Read the settings file, or `None` when it is absent or unusable.
#[must_use]
pub fn load() -> Option<cfg::Settings> {
    cfg::load(std::path::Path::new(cfg::PATH))
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
        std::path::Path::new(cfg::PATH),
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
