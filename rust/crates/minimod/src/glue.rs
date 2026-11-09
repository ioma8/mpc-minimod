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
