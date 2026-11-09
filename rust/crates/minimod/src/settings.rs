//! The looper's persisted settings, as safe values.

use crate::cfg;

/// Read the settings file, or `None` when it is absent or unusable.
#[must_use]
pub fn load() -> Option<cfg::Settings> {
    cfg::load(std::path::Path::new(cfg::PATH))
}

/// Write the settings file.
pub fn save(trim_frames: i32, master: u32) {
    let _ = cfg::save(
        std::path::Path::new(cfg::PATH),
        cfg::Settings {
            trim_frames,
            master: Some(master),
        },
    );
}

/// Load the persisted settings into the running mod.
pub fn load_into_mod() {
    let Some(settings) = load() else {
        return;
    };
    mpc_sample_sdk::audio::set_extra_latency(settings.trim_frames);
    if let Some(master) = settings.master {
        crate::backend::looper::looper_set_master(master);
    }
}
