//! The loader: finding the app, checking the firmware, and the pieces the
//! constructor needs before any hook is installed.

use crate::firmware;

/// The `.text` address the stock app's load segment must cover.
const TEXT_OFFSET: usize = 0xb5_5b00;
/// `PT_LOAD`.
const PT_LOAD: u32 = 1;

/// Two words of every site the mod patches, so a different firmware is refused
/// instead of hooked at the wrong offsets.
const FIRMWARE_SITES: [(usize, u32); 37] = [
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
    (0x03f3_98f0, 0xd503_233f),
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
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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

/// Read process memory, as the glue's own reader does.
///
/// # Safety
///
/// `dst` must be writable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rd(address: usize, dst: *mut u8, length: usize) -> i32 {
    if dst.is_null() || length == 0 {
        return 0;
    }
    // SAFETY: the caller promises the buffer.
    let buffer = unsafe { core::slice::from_raw_parts_mut(dst, length) };
    i32::from(firmware::read(address, buffer))
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
#[unsafe(no_mangle)]
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

/// The page's own paint, for the glue.
#[must_use]
pub fn original_paint() -> Option<unsafe extern "C" fn(usize, usize)> {
    // SAFETY: written once at install.
    unsafe { *core::ptr::addr_of!(ORIGINAL_PAINT) }
}

/// Install the paint hook in the screen's vtable.
///
/// # Safety
///
/// `base` must be the app's load address and `handler` the entry point to store.
#[unsafe(no_mangle)]
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
#[unsafe(no_mangle)]
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

/// The latency override, or -1 when the mod measures instead.
#[must_use]
pub fn fixed_latency() -> i32 {
    unsafe extern "C" {
        fn minimod_glue_lat_fixed() -> i32;
    }
    // SAFETY: the glue's own accessor.
    unsafe { minimod_glue_lat_fixed() }
}

/// The calibration trim, in frames.
#[must_use]
pub fn extra_latency() -> u32 {
    unsafe extern "C" {
        fn minimod_glue_lat_extra() -> u32;
    }
    // SAFETY: the glue's own accessor.
    unsafe { minimod_glue_lat_extra() }
}

/// The output ring's size in frames.
#[must_use]
pub fn buffer_out() -> u32 {
    unsafe extern "C" {
        fn minimod_glue_buf_out() -> u32;
    }
    // SAFETY: the glue's own accessor.
    unsafe { minimod_glue_buf_out() }
}

/// Record the output ring's size.
///
/// # Safety
///
/// The glue owns the value.
pub unsafe fn set_buffer_out(frames: u32) {
    unsafe extern "C" {
        fn minimod_glue_set_buf_out(frames: u32);
    }
    // SAFETY: the glue's own setter.
    unsafe { minimod_glue_set_buf_out(frames) }
}

/// The input ring's size in frames.
#[must_use]
pub fn buffer_in() -> u32 {
    unsafe extern "C" {
        fn minimod_glue_buf_in() -> u32;
    }
    // SAFETY: the glue's own accessor.
    unsafe { minimod_glue_buf_in() }
}

/// Record the input ring's size.
///
/// # Safety
///
/// The glue owns the value.
pub unsafe fn set_buffer_in(frames: u32) {
    unsafe extern "C" {
        fn minimod_glue_set_buf_in(frames: u32);
    }
    // SAFETY: the glue's own setter.
    unsafe { minimod_glue_set_buf_in(frames) }
}

/// The input delay the device reported.
#[must_use]
pub fn input_delay() -> u32 {
    unsafe extern "C" {
        fn minimod_glue_in_delay() -> u32;
    }
    // SAFETY: the glue's own accessor.
    unsafe { minimod_glue_in_delay() }
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
        handler: glue_buttons as *const (),
    },
    Site {
        offset: 0x03f3_98f0,
        handler: crate::audio::minimod_hook_audio_writer as *const (),
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
        handler: glue_midi_note as *const (),
    },
];

// The handlers that are still C.
unsafe extern "C" {
    fn ui_buttons(event: usize);
    fn midi_note_event(app: usize, context: usize, event: usize, source: usize);
    fn install_screen_paint() -> i32;
    fn minimod_glue_set_originals(originals: *const Originals);
    fn minimod_glue_finished(touched: i32);
}

/// The displaced functions the glue's own helpers still call.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Originals {
    /// The app's button dispatch.
    pub buttons: usize,
    /// The app's repaint.
    pub repaint: usize,
    /// The app's control-surface setter.
    pub control: usize,
    /// The app's MIDI note handler.
    pub midi_note: usize,
    /// The app's bass preview writer.
    pub bass_writer: usize,
    /// The slice command's constructor.
    pub hpss_ctor: usize,
    /// The slice command's execute method.
    pub hpss_exec: usize,
}

/// The button entry, as the glue installs it.
unsafe extern "C" fn glue_buttons(event: usize) {
    // SAFETY: the app calls this with its own event.
    unsafe { ui_buttons(event) };
}

/// The MIDI note event, as the glue installs it.
unsafe extern "C" fn glue_midi_note(app: usize, context: usize, event: usize, source: usize) {
    // SAFETY: the app calls this with its own event.
    unsafe { midi_note_event(app, context, event, source) };
}

/// How many sites the glue has to patch.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_site_count() -> usize {
    SITES.len()
}

/// The offset of one site.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_site_offset(index: usize) -> usize {
    SITES.get(index).map_or(0, |site| site.offset)
}

/// The handler for one site.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_site_handler(index: usize) -> *const () {
    SITES
        .get(index)
        .map_or(core::ptr::null(), |site| site.handler)
}

/// Whether the note-on hook was installed, which the preview needs.
static mut NOTE_ON_OK: bool = false;
/// Whether the audio writer hook was installed.
static mut WRITER_OK: bool = false;

/// Keep the displaced function for one site and hand it to whoever needs it.
///
/// # Safety
///
/// Called once per site, immediately after that site is patched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_site_record(index: usize, original: usize) {
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
            5 => crate::input::minimod_note_off_original(original),
            6 => crate::cc::minimod_hook_cc_original(crate::input::from_address(original)),
            7 => crate::entries::minimod_pitch_original(original),
            9 => {
                WRITER_OK = original != 0;
                crate::audio::minimod_audio_writer_original_now(original);
            }
            10 => crate::entries::minimod_repaint_original(original),
            11 => crate::entries::minimod_assign_original(original),
            12 => crate::hpss_exec::minimod_hpss_ctor_original(original),
            13 => crate::hpss_exec::minimod_hpss_exec_original(original),
            14 => crate::entries::minimod_writer_original(original),
            15 => crate::midi::minimod_midi_note_original(original),
            _ => {}
        }
    }
}

/// Hand the glue the originals its own helpers call, install the paint hook and seal.
///
/// # Safety
///
/// Called once, after every site has been patched and recorded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_after_sites(originals: *const Originals) -> i32 {
    if originals.is_null() {
        return 0;
    }
    // SAFETY: the caller's contract.
    unsafe {
        minimod_glue_set_originals(originals);
        install_screen_paint();
    }
    // SAFETY: the engine's own queries.
    let touched = unsafe { minimod_hooks_touched() != 0 };
    if touched {
        // SAFETY: sealing is the engine's own call.
        unsafe { minimod_seal_hooks() };
    }
    // The bass preview needs the note hooks and the writer, exactly as the C
    // constructor required.
    // SAFETY: the flags are this module's.
    unsafe {
        crate::bass_ffi::g_bass_preview_available = i32::from(NOTE_ON_OK && WRITER_OK);
        minimod_glue_finished(i32::from(touched));
    }
    i32::from(touched)
}

// The hook engine's own calls.
unsafe extern "C" {
    fn minimod_hooks_touched() -> i32;
    fn minimod_seal_hooks();
}

#[cfg(test)]
mod tests {
    use super::{FIRMWARE_SITES, PT_LOAD, TEXT_OFFSET};

    #[test]
    fn every_site_is_word_aligned_and_inside_the_image() {
        assert_eq!(FIRMWARE_SITES.len(), 37);
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
