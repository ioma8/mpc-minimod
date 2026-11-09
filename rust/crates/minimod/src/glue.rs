//! The loader: finding the app, checking the firmware, and the pieces the
//! constructor needs before any hook is installed.

use crate::firmware;
use crate::hooks;

/// The `.text` address the stock app's load segment must cover.
const TEXT_OFFSET: usize = 0xb5_5b00;
/// `PT_LOAD`.
const PT_LOAD: u32 = 1;

/// Two words of every site the mod patches, so a different firmware is refused
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

/// One program header, as aarch64 lays it out.
#[repr(C)]
struct ProgramHeader {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

/// One entry of the loader's module list, as far as this needs it.
#[repr(C)]
struct ModuleInfo {
    address: usize,
    name: *const u8,
    headers: *const ProgramHeader,
    count: u16,
}

unsafe extern "C" {
    /// Walk the loaded modules.
    fn dl_iterate_phdr(
        callback: Option<unsafe extern "C" fn(*mut ModuleInfo, usize, *mut usize) -> i32>,
        data: *mut usize,
    ) -> i32;
}

/// Whether the firmware matches every site the mod patches.
///
/// # Safety
///
/// `base` must be the app's load address.
#[must_use]
pub unsafe extern "C" fn minimod_firmware_ok(base: usize) -> i32 {
    for (offset, expected) in FIRMWARE_SITES {
        let Some(word) = firmware::read_u32(base + offset) else {
            return 0;
        };
        if word != expected {
            return 0;
        }
    }
    1
}

/// The stock app's load address, or zero when it is not loaded.
pub extern "C" fn minimod_find_main() -> usize {
    let mut found: usize = 0;
    // SAFETY: the callback only writes to `found`, which outlives the walk.
    unsafe {
        dl_iterate_phdr(Some(find_main), core::ptr::from_mut(&mut found));
    }
    found
}

/// The loader's callback: the first module with no name that covers `.text`.
///
/// # Safety
///
/// Called by the loader with a valid module and the caller's data pointer.
unsafe extern "C" fn find_main(info: *mut ModuleInfo, _size: usize, out: *mut usize) -> i32 {
    if info.is_null() || out.is_null() {
        return 0;
    }
    // SAFETY: the loader passes a valid entry.
    let info = unsafe { &*info };
    let name = info.name;
    // SAFETY: the name is a NUL-terminated C string or null.
    let unnamed = name.is_null() || unsafe { *name } == 0;
    if !unnamed {
        return 0;
    }
    for index in 0..usize::from(info.count) {
        // SAFETY: the header array holds `count` entries.
        let header = unsafe { &*info.headers.add(index) };
        if header.kind != PT_LOAD {
            continue;
        }
        let start = usize::try_from(header.vaddr).unwrap_or(0);
        let size = usize::try_from(header.memsz).unwrap_or(0);
        if TEXT_OFFSET >= start && TEXT_OFFSET < start.saturating_add(size) {
            // SAFETY: the caller passed a writable slot.
            unsafe { *out = info.address };
            return 1;
        }
    }
    0
}

/// Whether an address lies inside any mapping.
///
/// The glue asks this about the app's read-only pages too - the screen's vtable
/// among them - so this keeps every range, not only the writable ones.  Unknown
/// stays permissive: the reads and writes fail safely on their own.
///
/// # Safety
///
/// The pointer is only compared, never dereferenced.
pub unsafe extern "C" fn mapped(address: *const u8) -> i32 {
    use std::sync::OnceLock;
    static RANGES: OnceLock<Vec<(usize, usize)>> = OnceLock::new();
    let ranges = RANGES.get_or_init(|| {
        let Ok(text) = std::fs::read_to_string("/proc/self/maps") else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| {
                let span = line.split_whitespace().next()?;
                let (low, high) = span.split_once('-')?;
                Some((
                    usize::from_str_radix(low, 16).ok()?,
                    usize::from_str_radix(high, 16).ok()?,
                ))
            })
            .collect()
    });
    if ranges.is_empty() {
        return 1;
    }
    let address = address as usize;
    i32::from(
        ranges
            .iter()
            .any(|&(low, high)| address >= low && address < high),
    )
}

/// The page's own paint, displaced by the mod.
static mut ORIGINAL_PAINT: Option<unsafe extern "C" fn(usize, usize)> = None;

/// Install the paint hook in the screen's vtable.
///
/// # Safety
///
/// `base` must be the app's load address and `handler` the entry point to store.
pub unsafe extern "C" fn minimod_paint_install(base: usize, handler: *const ()) -> i32 {
    let address = base + 0x6e2_12a0;
    if unsafe { !is_mapped(address as *const u8) } {
        return 0;
    }
    // SAFETY: the address is inside the app's screen vtable.
    let slot = unsafe { (address as *mut usize).add(27) };
    // SAFETY: see above.
    let original = unsafe { *slot };
    if original == 0 {
        return 0;
    }
    // SAFETY: the original is the app's own paint.
    unsafe {
        ORIGINAL_PAINT =
            core::mem::transmute::<usize, Option<unsafe extern "C" fn(usize, usize)>>(original);
    }
    let page = address & !0xfff;
    // SAFETY: the page is inside the app's image.
    let unlocked = unsafe {
        libc::mprotect(
            page as *mut libc::c_void,
            0x1000,
            libc::PROT_READ | libc::PROT_WRITE,
        )
    } == 0;
    if !unlocked {
        return 0;
    }
    // SAFETY: the page is writable now.
    unsafe {
        *slot = handler as usize;
        libc::mprotect(page as *mut libc::c_void, 0x1000, libc::PROT_READ);
    }
    1
}

/// The paint the mod displaced, for the glue.
#[must_use]
pub extern "C" fn minimod_paint_original() -> Option<unsafe extern "C" fn(usize, usize)> {
    // SAFETY: written once at install.
    unsafe { *core::ptr::addr_of!(ORIGINAL_PAINT) }
}

/// The repaint the mod displaced, for the pump.
#[must_use]
pub fn original_repaint() -> Option<unsafe extern "C" fn(usize, u64, u64)> {
    // SAFETY: written once at install.
    unsafe { *core::ptr::addr_of!(crate::entries::ORIGINAL_REPAINT) }
}

/// The glue's mapping test, from the crate's side.
///
/// # Safety
///
/// The pointer is only compared.
unsafe fn is_mapped(address: *const u8) -> bool {
    // SAFETY: the glue's own test.
    unsafe { mapped(address) != 0 }
}

/// The output ring's size in frames, as last read from the device (the device tests read it by name).
#[unsafe(no_mangle)]
pub static mut g_buf_out: u32 = 0;

/// The calibration trim, in frames, never negative.
#[must_use]
pub fn extra_latency() -> u32 {
    // SAFETY: written only by the UI thread and the config load.
    u32::try_from(unsafe { g_lat_extra }).unwrap_or(0)
}

/// The output ring's size in frames.
#[must_use]
pub fn buffer_out() -> u32 {
    // SAFETY: a word written by the audio thread that owns the stream.
    unsafe { g_buf_out }
}

/// Record the output ring's size.
///
/// # Safety
///
/// Only the audio thread that owns the stream may call this.
pub unsafe fn set_buffer_out(frames: u32) {
    // SAFETY: the caller's contract.
    unsafe { g_buf_out = frames };
}

/// The input ring's size in frames.
#[must_use]
pub fn buffer_in() -> u32 {
    // SAFETY: a word written by the audio thread that owns the stream.
    unsafe { crate::audio::g_buf_in }
}

/// Record the input ring's size.
///
/// # Safety
///
/// Only the audio thread that owns the stream may call this.
pub unsafe fn set_buffer_in(frames: u32) {
    // SAFETY: the caller's contract.
    unsafe { crate::audio::g_buf_in = frames };
}

/// The input delay the device reported.
#[must_use]
pub fn input_delay() -> u32 {
    // SAFETY: a word written by the audio thread that owns the stream.
    unsafe { crate::audio::G_IN_DELAY }
}

/// One site the mod patches: its image offset and the handler to put there.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Site {
    /// Offset from the app's load address.
    pub offset: usize,
    /// The entry point to install.
    pub handler: *const (),
}

// The handler pointers are code addresses, which never move.
unsafe impl Sync for Site {}
unsafe impl Send for Site {}

/// Everything the constructor patches, in the glue's order, with the offsets read
/// from the glue's own defines.
static SITES: [Site; 16] = [
    Site {
        offset: 0x0154_5a50,
        handler: crate::entries::minimod_hook_sixteen_ctor as *const (),
    },
    Site {
        offset: 0x0214_72f0,
        handler: crate::entries::minimod_hook_sixteen_proc as *const (),
    },
    Site {
        offset: 0x0140_dbc0,
        handler: crate::input::minimod_hook_data as *const (),
    },
    Site {
        offset: 0x01e2_7360,
        handler: crate::input::minimod_hook_note_on as *const (),
    },
    Site {
        offset: 0x023a_ff60,
        handler: crate::input::minimod_hook_surface as *const (),
    },
    Site {
        offset: 0x01e2_70d0,
        handler: crate::input::minimod_hook_note_off as *const (),
    },
    Site {
        offset: 0x0159_f230,
        handler: crate::cc::minimod_hook_cc as *const (),
    },
    Site {
        offset: 0x027e_4400,
        handler: crate::entries::minimod_hook_pitch_off as *const (),
    },
    Site {
        offset: 0x0159_f2f0,
        handler: crate::ui::minimod_ui_buttons as *const (),
    },
    Site {
        offset: 0x0114_9880,
        handler: crate::entries::minimod_hook_repaint as *const (),
    },
    Site {
        offset: 0x00f5_35c0,
        handler: crate::entries::minimod_hook_str_assign as *const (),
    },
    Site {
        offset: 0x0143_ed40,
        handler: crate::hpss_exec::minimod_hpss_ctor_entry as *const (),
    },
    Site {
        offset: 0x0143_f0b0,
        handler: crate::hpss_exec::minimod_hpss_exec_entry as *const (),
    },
    Site {
        offset: 0x01d8_aff0,
        handler: crate::entries::minimod_hook_hpss_writer as *const (),
    },
    Site {
        offset: 0x0211_b090,
        handler: crate::midi::minimod_hook_midi_note as *const (),
    },
    Site {
        offset: 0x0196_8020,
        handler: crate::autotrim::minimod_hook_set_control as *const (),
    },
];

/// Whether the note-on hook was installed, which the preview needs.
static mut NOTE_ON_OK: bool = false;
/// Whether the note-off hook was installed.
static mut NOTE_OFF_OK: bool = false;

/// Keep the displaced function for one site and hand it to whoever needs it.
///
/// # Safety
///
/// Called once per site, immediately after that site is patched.
unsafe fn record_site(index: usize, original: usize) -> bool {
    // SAFETY: the caller's contract.
    unsafe {
        match index {
            0 => crate::entries::minimod_ctor_original(original),
            1 => crate::entries::minimod_proc_original(original),
            2 => crate::input::minimod_data_original(original),
            3 => {
                NOTE_ON_OK = original != 0;
                crate::input::minimod_note_on_original(original);
            }
            4 => crate::input::minimod_surface_original(original),
            5 => {
                NOTE_OFF_OK = original != 0;
                crate::input::minimod_note_off_original(original);
            }
            6 => crate::cc::minimod_hook_cc_original(crate::input::from_address(original)),
            7 => crate::entries::minimod_pitch_original(original),
            8 => crate::ui::minimod_buttons_original(original),
            9 => crate::entries::minimod_repaint_original(original),
            10 => crate::entries::minimod_assign_original(original),
            11 => crate::hpss_exec::minimod_hpss_ctor_original(original),
            12 => crate::hpss_exec::minimod_hpss_exec_original(original),
            13 => crate::entries::minimod_writer_original(original),
            14 => crate::midi::minimod_midi_note_original(original),
            15 => crate::autotrim::record_set_control(original),
            _ => return false,
        }
    }
    true
}

/// Install the paint hook, seal the patched pages and say whether anything landed.
///
/// # Safety
///
/// Called once, after every site has been patched and recorded.
unsafe fn after_sites() -> bool {
    let _ = crate::paint::minimod_paint_install_self();
    // SAFETY: the engine's own queries.
    let touched = hooks::minimod_hooks_touched() != 0;
    if touched {
        // SAFETY: sealing is the engine's own call.
        hooks::minimod_seal_hooks();
    }
    // The bass preview needs the note hooks; it mixes into the output tap.
    // SAFETY: the flags are this module's.
    unsafe {
        crate::bass_ffi::G_BASS_PREVIEW_AVAILABLE = i32::from(NOTE_ON_OK && NOTE_OFF_OK);
    }
    touched
}

/// The calibration trim the config file carries.
#[unsafe(no_mangle)]
pub static mut g_lat_extra: i32 = 0;

/// Read the saved calibration and apply it.
fn load_config() {
    let mut trim = 0_i32;
    let mut master = -1_i32;
    // SAFETY: both pointers are this frame's.
    if unsafe { crate::cfg_ffi::minimod_cfg_load(&raw mut trim, &raw mut master) } == 0 {
        return;
    }
    // SAFETY: the static is this module's.
    unsafe { g_lat_extra = trim };
    if master >= 0 {
        crate::looper_ffi::looper_set_master(u32::try_from(master).unwrap_or(0));
    }
}

/// Install the mod: find the app, check the image, patch every site.
pub extern "C" fn minimod_load() {
    let found = minimod_find_main();
    if found == 0 {
        return;
    }
    firmware::minimod_set_base(found);
    // SAFETY: the image check reads the mapped file.
    let ok = unsafe { minimod_firmware_ok(found) };
    if ok == 0 {
        return;
    }
    load_config();
    patch_sites(found);
}

/// Patch the sites the table names, in order.
fn patch_sites(base: usize) {
    for (index, entry) in SITES.iter().enumerate() {
        let site = base + entry.offset;
        let handler = entry.handler as usize;
        // SAFETY: the site is a checked image offset and the handler is ours.
        let trampoline = unsafe { hooks::minimod_install_hook(base, site, handler) };
        // SAFETY: the recorders take this site's number.
        let recorded = unsafe { record_site(index, trampoline as usize) };
        debug_assert!(recorded, "site {index} has no recorder");
    }
    // SAFETY: every site is patched and recorded.
    unsafe { after_sites() };
}

/// The loader runs this when the app maps the library.
#[cfg(all(target_os = "linux", not(test)))]
#[used]
#[unsafe(link_section = ".init_array")]
static LOAD: extern "C" fn() = minimod_load;

#[cfg(test)]
mod tests {
    use super::{FIRMWARE_SITES, PT_LOAD, TEXT_OFFSET};

    #[test]
    fn every_site_has_a_recorder_for_its_displaced_function() {
        // A site whose original is never recorded is a hook that swallows its
        // event: the button dispatcher lost its recorder once and CHOP died.
        for index in 0..super::SITES.len() {
            // SAFETY: recording stores an address; nothing is called.
            assert!(
                unsafe { super::record_site(index, 0) },
                "site {index} has no recorder"
            );
        }
    }

    #[test]
    fn every_site_is_word_aligned_and_inside_the_image() {
        assert_eq!(FIRMWARE_SITES.len(), 36);
        for (offset, _) in FIRMWARE_SITES {
            assert_eq!(offset % 4, 0, "site {offset:#x} is not aligned");
            assert!(
                offset < 0x0400_0000,
                "site {offset:#x} is outside the image"
            );
        }
        assert_eq!(PT_LOAD, 1);
        assert_eq!(TEXT_OFFSET, 0xb5_5b00);
    }
}
