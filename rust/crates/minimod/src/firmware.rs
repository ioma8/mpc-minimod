//! The stock app as a loaded image: where it is mapped, how to reach into it,
//! and how to read and write its memory.
//!
//! Everything the mod does to the firmware goes through this module: the
//! address of a hooked site, the bytes of a function prologue, the fields of an
//! object the app owns.  Memory access deliberately goes through
//! `/proc/self/mem` instead of a raw pointer: it cannot fault when the app
//! unmaps a page underneath us, and it sidesteps the aliasing rules a Rust
//! reference to arbitrary firmware memory would have to violate.
//!
//! Two descriptors are kept: on this kernel a *read* through an `O_RDWR`
//! `/proc/self/mem` returns `EIO`, which silently breaks every read, so reads
//! and writes open the file separately.

/// First instruction of every site this build hooks, read from firmware
/// 1.3.0.12.  If any of them does not match, nothing is installed: patching an
/// offset in a different build would overwrite a random instruction.
pub const KNOWN_BUILD_SITES: &[(usize, u32)] = &[
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

/// Where the stock app is loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Firmware {
    base: usize,
}

impl Firmware {
    /// The app's image at `base`, as reported by the dynamic loader.
    #[must_use]
    pub const fn from_base(base: usize) -> Self {
        Self { base }
    }

    /// Find the stock app among the loaded images.
    ///
    /// The app is the only executable without a name whose text segment covers
    /// the probe offset inside the tested firmware; a library never does.
    #[must_use]
    pub fn find() -> Option<Self> {
        platform::find_base().map(Self::from_base)
    }

    /// Load address of the image.
    #[must_use]
    pub const fn base(self) -> usize {
        self.base
    }

    /// Absolute address of `offset` in the image.
    #[must_use]
    pub const fn addr(self, offset: usize) -> usize {
        self.base.wrapping_add(offset)
    }

    /// Read the 32-bit word at `offset`.
    #[must_use]
    pub fn word(self, offset: usize) -> Option<u32> {
        read_u32(self.addr(offset))
    }

    /// Read the 64-bit word at `offset`.
    #[must_use]
    pub fn qword(self, offset: usize) -> Option<u64> {
        read_u64(self.addr(offset))
    }

    /// Read the pointer stored at `offset`; zero reads as `None`.
    #[must_use]
    pub fn pointer(self, offset: usize) -> Option<usize> {
        self.qword(offset)
            .filter(|value| *value != 0)
            .map(|value| value as usize)
    }

    /// Write the 32-bit word at `offset`.
    #[must_use]
    pub fn set_word(self, offset: usize, value: u32) -> bool {
        write(self.addr(offset), &value.to_le_bytes())
    }

    /// Check that every hooked site still holds the first instruction the
    /// tested build has there.
    ///
    /// # Errors
    ///
    /// Returns the first site that does not match, or that cannot be read.
    pub fn check_known_build(self) -> Result<(), SiteMismatch> {
        for &(offset, expected) in KNOWN_BUILD_SITES {
            match self.word(offset) {
                Some(word) if word == expected => {}
                Some(found) => {
                    return Err(SiteMismatch {
                        offset,
                        expected,
                        found: Some(found),
                    });
                }
                None => {
                    return Err(SiteMismatch {
                        offset,
                        expected,
                        found: None,
                    });
                }
            }
        }
        Ok(())
    }
}

/// A hooked site whose first instruction is not the expected one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SiteMismatch {
    /// Offset of the site in the image.
    pub offset: usize,
    /// The instruction the tested build has there.
    pub expected: u32,
    /// What is there now, or `None` when it could not be read.
    pub found: Option<u32>,
}

impl core::fmt::Display for SiteMismatch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.found {
            Some(found) => write!(
                f,
                "site {:#010x} holds {found:#010x}, expected {:#010x}",
                self.offset, self.expected
            ),
            None => write!(
                f,
                "site {:#010x} is unreadable, expected {:#010x}",
                self.offset, self.expected
            ),
        }
    }
}

impl std::error::Error for SiteMismatch {}

/// Read exactly `out.len()` bytes of process memory.
#[must_use]
pub fn read(address: usize, out: &mut [u8]) -> bool {
    platform::read(address, out)
}

/// Write all of `bytes` to process memory.
#[must_use]
pub fn write(address: usize, bytes: &[u8]) -> bool {
    platform::write(address, bytes)
}

/// Read one 32-bit word of process memory.
/// A firmware function taking one pointer and returning nothing.
pub type VoidOfPointer = unsafe extern "C" fn(usize);
/// A firmware function taking a pointer and a value.
pub type VoidOfPointerValue = unsafe extern "C" fn(usize, u32);
/// A firmware function taking two pointers.
pub type VoidOfTwoPointers = unsafe extern "C" fn(usize, usize);
/// A firmware function taking three pointers.
pub type VoidOfThreePointers = unsafe extern "C" fn(usize, usize, usize);
/// A firmware function taking a pointer and a slot index, returning the index
/// the app assigned.
pub type IntOfPointerSlots = unsafe extern "C" fn(usize, u32, u32, u32) -> i32;

/// Call a firmware function taking one pointer and returning nothing.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_pointer(address: usize, argument: usize) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfPointer = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(argument) };
}

/// Call a firmware function taking two pointers.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_two_pointers(address: usize, first: usize, second: usize) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfTwoPointers = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(first, second) };
}

/// Call a firmware function taking three pointers.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_three_pointers(address: usize, first: usize, second: usize, third: usize) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfThreePointers = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(first, second, third) };
}

/// Call a firmware function taking a pointer and a value.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_pointer_value(address: usize, pointer: usize, value: u32) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfPointerValue = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(pointer, value) };
}

/// Call the app's slice insert.
///
/// # Safety
///
/// `address` must be the app's insert entry and the arguments must be valid for
/// it.
#[must_use]
pub unsafe fn insert_slice(
    address: usize,
    continuation: usize,
    start: u32,
    count: u32,
    frames: u32,
) -> i32 {
    // SAFETY: the caller guarantees the address and the signature.
    let function: IntOfPointerSlots = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(continuation, start, count, frames) }
}

/// A firmware function taking one pointer and returning an `int`.
pub type IntOfPointer = unsafe extern "C" fn(usize) -> i32;

/// Call the firmware function at `address` with one pointer argument.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature,
/// and `argument` must be a valid argument for it.
#[must_use]
pub unsafe fn int_of_pointer(address: usize, argument: usize) -> i32 {
    // SAFETY: the caller guarantees the address and the signature.
    let function: IntOfPointer = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(argument) }
}

/// Read one 32-bit word of process memory.
#[must_use]
pub fn read_u32(address: usize) -> Option<u32> {
    let mut buffer = [0_u8; 4];
    read(address, &mut buffer).then(|| u32::from_le_bytes(buffer))
}

/// Read one 64-bit word of process memory.
#[must_use]
pub fn read_u64(address: usize) -> Option<u64> {
    let mut buffer = [0_u8; 8];
    read(address, &mut buffer).then(|| u64::from_le_bytes(buffer))
}

/// Read a pointer-sized value; zero reads as `None`.
#[must_use]
pub fn read_pointer(address: usize) -> Option<usize> {
    read_u64(address)
        .filter(|value| *value != 0)
        .map(|value| value as usize)
}

/// The parts that only exist on the device's platform.
///
/// The host builds (tests, clippy) must still compile, so the Linux-only
/// machinery — the dynamic loader walk and `/proc/self/mem` — lives behind this
/// module and reports failure everywhere else.
#[cfg(target_os = "linux")]
mod platform {
    use std::fs::File;
    use std::os::raw::c_void;
    use std::os::unix::fs::FileExt;
    use std::sync::OnceLock;

    /// Offset inside the stock app's text segment, used to recognise which
    /// loaded image is the app.  Any PT_LOAD that covers it belongs to firmware
    /// 1.3.0.12.
    const APP_PROBE_OFFSET: usize = 0x00b5_5b00;

    struct Search {
        base: Option<usize>,
    }

    /// `dl_iterate_phdr` callback: remember the unnamed image that covers the
    /// probe offset, and stop iterating.
    ///
    /// # Safety
    ///
    /// Called by the dynamic loader with a valid `info` and with `data` pointing
    /// at the [`Search`] that outlives the iteration.
    unsafe extern "C" fn find_main(
        info: *mut libc::dl_phdr_info,
        _size: usize,
        data: *mut c_void,
    ) -> i32 {
        let search = unsafe { &mut *data.cast::<Search>() };
        let info = unsafe { &*info };
        if !info.dlpi_name.is_null() && unsafe { *info.dlpi_name } != 0 {
            return 0;
        }
        let count = usize::from(info.dlpi_phnum);
        if info.dlpi_phdr.is_null() || count == 0 {
            return 0;
        }
        // SAFETY: the loader provides `dlpi_phnum` program headers at `dlpi_phdr`.
        let headers = unsafe { core::slice::from_raw_parts(info.dlpi_phdr, count) };
        for header in headers {
            if header.p_type != libc::PT_LOAD {
                continue;
            }
            let start = header.p_vaddr as usize;
            let end = start.saturating_add(header.p_memsz as usize);
            if (start..end).contains(&APP_PROBE_OFFSET) {
                search.base = Some(info.dlpi_addr as usize);
                return 1;
            }
        }
        0
    }

    /// Load address of the stock app, or `None` when it is not the image that
    /// was loaded.
    pub(super) fn find_base() -> Option<usize> {
        let mut search = Search { base: None };
        // SAFETY: the callback only reads the `dl_phdr_info` the loader hands it
        // and writes into `search`, which outlives the call.
        let found = unsafe {
            libc::dl_iterate_phdr(
                Some(find_main),
                std::ptr::from_mut(&mut search).cast::<c_void>(),
            )
        };
        if found == 0 {
            return None;
        }
        search.base
    }

    pub(super) fn read(address: usize, out: &mut [u8]) -> bool {
        let Some(file) = reader() else {
            return false;
        };
        file.read_at(out, address as u64)
            .is_ok_and(|got| got == out.len())
    }

    pub(super) fn write(address: usize, bytes: &[u8]) -> bool {
        let Some(file) = writer() else {
            return false;
        };
        file.write_at(bytes, address as u64)
            .is_ok_and(|put| put == bytes.len())
    }

    fn reader() -> Option<&'static File> {
        static FILE: OnceLock<Option<File>> = OnceLock::new();
        FILE.get_or_init(|| File::open("/proc/self/mem").ok())
            .as_ref()
    }

    fn writer() -> Option<&'static File> {
        static FILE: OnceLock<Option<File>> = OnceLock::new();
        FILE.get_or_init(|| {
            std::fs::OpenOptions::new()
                .write(true)
                .open("/proc/self/mem")
                .ok()
        })
        .as_ref()
    }
}

/// Everywhere else (host tests, clippy) there is no firmware to reach into.
#[cfg(not(target_os = "linux"))]
mod platform {
    pub(super) fn find_base() -> Option<usize> {
        None
    }

    pub(super) fn read(_address: usize, _out: &mut [u8]) -> bool {
        false
    }

    pub(super) fn write(_address: usize, _bytes: &[u8]) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{Firmware, KNOWN_BUILD_SITES};

    #[test]
    fn the_site_table_is_sorted_and_unique() {
        assert_eq!(KNOWN_BUILD_SITES.len(), 37);
        let mut offsets: Vec<usize> = KNOWN_BUILD_SITES.iter().map(|(o, _)| *o).collect();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), KNOWN_BUILD_SITES.len(), "duplicate site");
    }

    #[test]
    fn every_site_starts_with_a_prologue_instruction() {
        // `paciasp` and `bti c`, plus the one site that starts with a `str`.
        for &(offset, word) in KNOWN_BUILD_SITES {
            assert!(
                word == 0xd503_233f || word == 0xd503_245f || word == 0x3940_2003,
                "site {offset:#010x} has an unexpected prologue {word:#010x}"
            );
        }
    }

    #[test]
    fn addresses_are_relative_to_the_load_base() {
        let firmware = Firmware::from_base(0xaaaa_aaaa_0000);
        assert_eq!(firmware.addr(0x1234), 0xaaaa_aaaa_1234);
        assert_eq!(Firmware::from_base(0).addr(0x10), 0x10);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reading_our_own_stack_works_and_unmapped_memory_does_not() {
        let value = 0x1234_5678_9abc_def0_u64;
        let address = std::ptr::from_ref(&value) as usize;
        assert_eq!(super::read_u64(address), Some(value));
        // A page that is certainly not mapped: the read must fail, not fault.
        assert_eq!(super::read_u32(1), None);
    }
}
