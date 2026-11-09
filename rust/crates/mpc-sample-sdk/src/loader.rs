//! The mod lifecycle: find the app, verify that it is the tested firmware,
//! install the SDK's hooks, and hand control to the mod.
//!
//! A mod does not call any of this itself; it emits the [`crate::export_mod!`]
//! entry point once and implements [`crate::Mod`].

use core::fmt;

use crate::event::Mod;
use crate::image::{self, Firmware};
use crate::sites;

/// Two words of every site the SDK patches, so a different firmware is refused
/// instead of hooked at the wrong offsets.
const FIRMWARE_SITES: [(usize, u32); 36] = [
    (0x00f5_35c0, 0xd503_233f),
    (0x0114_9880, 0xd503_233f),
    (0x013b_b930, 0xd503_233f),
    (0x0140_dbc0, 0xd503_233f),
    (0x0143_ed40, 0xd503_233f),
    (0x0143_f0b0, 0xd503_233f),
    (0x014a_7580, 0xd503_233f),
    (0x0154_5a50, 0xd503_233f),
    (0x0159_f230, 0x3940_2003),
    (0x0159_f2f0, 0xd503_245f),
    (0x015e_70c0, 0xd503_233f),
    (0x016b_6060, 0xd503_245f),
    (0x0192_d040, 0xd503_233f),
    (0x0195_cf40, 0xd503_233f),
    (0x0195_d1c0, 0xd503_233f),
    (0x0196_0af0, 0xd503_233f),
    (0x0196_0d50, 0xd503_233f),
    (0x0196_8020, 0xd503_233f),
    (0x01d8_aff0, 0xd503_233f),
    (0x01e2_70d0, 0xd503_233f),
    (0x01e2_7360, 0xd503_233f),
    (0x020a_3b10, 0xd503_233f),
    (0x020a_7140, 0xd503_233f),
    (0x020a_9fa0, 0xd503_233f),
    (0x020a_a9e0, 0xd503_233f),
    (0x0211_b090, 0xd503_233f),
    (0x0214_72f0, 0xd503_233f),
    (0x023a_ff60, 0xd503_233f),
    (0x027e_4400, 0xd503_233f),
    (0x0280_4e90, 0xd503_233f),
    (0x0280_dca0, 0xd503_233f),
    (0x0281_00c0, 0xd503_233f),
    (0x0281_2f10, 0xd503_245f),
    (0x0281_4ec0, 0xd503_233f),
    (0x0282_2260, 0xd503_233f),
    (0x0294_1ff0, 0xd503_245f),
];

/// Why a mod could not be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The stock app is not loaded.
    NotFound,
    /// The loaded image is not the firmware this SDK was built against.
    WrongFirmware,
    /// Not one hook site could be installed.
    NoHooks,
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the MPC Sample is not loaded"),
            Self::WrongFirmware => f.write_str("unsupported firmware image"),
            Self::NoHooks => f.write_str("no hook site could be installed"),
        }
    }
}

impl std::error::Error for InstallError {}

/// The installed mod, if any.  Written once from the constructor, read from the
/// app's threads; the mod is responsible for making its own state safe for the
/// threads that reach it.
static mut MODULE: Option<*mut dyn Mod> = None;

/// Run `f` with the installed mod.
pub fn with_mod<R>(f: impl FnOnce(&mut dyn Mod) -> R) -> Option<R> {
    // SAFETY: the pointer is set once before the app threads start and never
    // changes afterwards.
    unsafe {
        let module = *core::ptr::addr_of!(MODULE);
        module.map(|pointer| f(&mut *pointer))
    }
}

/// Whether every site word matches the tested firmware.
#[must_use]
pub fn firmware_ok(base: usize) -> bool {
    for (offset, expected) in FIRMWARE_SITES {
        let Some(word) = image::read_u32(base + offset) else {
            return false;
        };
        if word != expected {
            return false;
        }
    }
    true
}

/// The stock app's load address, or zero when it is not loaded.
#[must_use]
pub fn find_main() -> usize {
    Firmware::find().map_or(0, Firmware::base)
}

/// Install `module`: find and verify the firmware, install the hooks, and call
/// [`Mod::on_start`].
///
/// # Errors
///
/// Returns [`InstallError`] when the app is missing, the image is not the tested
/// firmware, or no hook site could be installed.
///
/// # Safety
///
/// Must run once, from the loader's `.init_array`, before the app's threads
/// start.  The module must stay alive for the life of the process.
pub unsafe fn install(module: &'static mut dyn Mod) -> Result<(), InstallError> {
    let Some(firmware) = Firmware::find() else {
        return Err(InstallError::NotFound);
    };
    image::set_base(firmware.base());
    if !firmware_ok(firmware.base()) {
        return Err(InstallError::WrongFirmware);
    }
    // SAFETY: this is the one write to the module slot.
    unsafe { MODULE = Some(module) };
    // Keep the ALSA taps linked; they are resolved by name by the app.
    crate::audio::keep_exports();
    if !sites::install(firmware.base()) {
        // SAFETY: nothing has observed the module yet.
        unsafe { MODULE = None };
        return Err(InstallError::NoHooks);
    }
    module.on_start();
    Ok(())
}

/// Emit the `.init_array` entry point for a mod type.
///
/// The type must implement [`crate::Mod`] and [`Default`].
#[macro_export]
macro_rules! export_mod {
    ($ty:ty) => {
        #[used]
        #[cfg_attr(target_os = "linux", unsafe(link_section = ".init_array"))]
        static MPC_SDK_ENTRY: extern "C" fn() = {
            extern "C" fn start() {
                static mut MODULE: ::core::option::Option<$ty> = None;
                // SAFETY: the loader runs this once, before the app's threads.
                let module: &'static mut $ty = unsafe {
                    let slot = &mut *::core::ptr::addr_of_mut!(MODULE);
                    slot.get_or_insert_with(<$ty>::default)
                };
                // SAFETY: as above; the module lives for the process.
                let _ = unsafe { $crate::install(module) };
            }
            start
        };
    };
}

#[cfg(test)]
mod tests {
    use super::firmware_ok;

    #[test]
    fn a_zero_image_is_not_the_firmware() {
        assert!(!firmware_ok(0));
    }
}
