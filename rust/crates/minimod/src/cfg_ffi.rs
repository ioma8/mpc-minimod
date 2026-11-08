//! C ABI of the looper's persisted settings.

use crate::cfg;

/// Read the settings file.
///
/// Writes the trim in frames and the volume (-1 when the file had none) and
/// returns 1 when anything was applied, 0 when the file is absent or unusable.
///
/// # Safety
///
/// `trim_frames` and `master` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_cfg_load(trim_frames: *mut i32, master: *mut i32) -> i32 {
    if trim_frames.is_null() || master.is_null() {
        return 0;
    }
    let Some(settings) = cfg::load(std::path::Path::new(cfg::PATH)) else {
        return 0;
    };
    // SAFETY: the caller promises both pointers are writable.
    unsafe {
        *trim_frames = settings.trim_frames;
        *master = settings
            .master
            .map_or(-1, |value| i32::try_from(value).unwrap_or(-1));
    }
    1
}

/// Write the settings file.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_cfg_save(trim_frames: i32, master: u32) {
    let _ = cfg::save(
        std::path::Path::new(cfg::PATH),
        cfg::Settings {
            trim_frames,
            master: Some(master),
        },
    );
}
