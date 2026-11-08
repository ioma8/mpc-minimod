//! C ABI of the sample-path resolution.

use core::ptr;
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::paths;

/// Copy `path` into a C buffer of `size` bytes, NUL-terminated.
fn copy_path(path: &Path, out: *mut c_char, size: usize) -> bool {
    if out.is_null() || size == 0 {
        return false;
    }
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() + 1 > size {
        return false;
    }
    // SAFETY: the caller promises `out` is writable for `size` bytes.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), out, bytes.len());
        *out.add(bytes.len()) = 0;
    }
    true
}

/// The path the sample object points at directly.
///
/// # Safety
///
/// `out` must be writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn path_from_object(
    sample: usize,
    want_frames: u32,
    out: *mut c_char,
    size: usize,
) -> i32 {
    match paths::from_object(sample, want_frames) {
        // SAFETY: see the function contract.
        Some(path) => i32::from(copy_path(&path, out, size)),
        None => 0,
    }
}

/// The path found by walking the object's pointer graph.
///
/// # Safety
///
/// `out` must be writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wav_path_of(
    object: usize,
    want_frames: u32,
    out: *mut c_char,
    size: usize,
) -> i32 {
    match paths::from_object_graph(object, want_frames) {
        // SAFETY: see the function contract.
        Some(path) => i32::from(copy_path(&path, out, size)),
        None => 0,
    }
}

/// The file that backs the selected sample: object path, graph scan, then the
/// unique recording with the same geometry.
///
/// # Safety
///
/// `path` must be writable for `path_size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hpss_path_for(
    sample: usize,
    frames: u32,
    channels: u32,
    rate: u32,
    path: *mut c_char,
    path_size: usize,
) -> i32 {
    let Ok(channels) = u16::try_from(channels) else {
        return 0;
    };
    match paths::for_sample(sample, frames, channels, rate) {
        // SAFETY: see the function contract.
        Some(found) => i32::from(copy_path(&found, path, path_size)),
        None => 0,
    }
}

/// The newest WAV under the temp project with this frame count.
///
/// # Safety
///
/// `out` must be writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn find_wav_by_frames(
    want_frames: u32,
    out: *mut c_char,
    size: usize,
) -> i32 {
    match paths::newest(Path::new(paths::TEMP_PROJECTS), want_frames) {
        // SAFETY: see the function contract.
        Some(path) => i32::from(copy_path(&path, out, size)),
        None => 0,
    }
}

/// The bytes `copy_path` writes, for the test below.
#[cfg(test)]
fn path_bytes(path: &Path) -> Vec<u8> {
    let mut buffer = [0_i8; 64];
    assert!(copy_path(path, buffer.as_mut_ptr().cast(), buffer.len()));
    // SAFETY: `copy_path` NUL-terminated the buffer.
    let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr().cast()) };
    text.to_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn paths_are_nul_terminated_and_bounded() {
        let path = PathBuf::from("/data/TempProjects/x/y.wav");
        let bytes = super::path_bytes(&path);
        assert_eq!(bytes, b"/data/TempProjects/x/y.wav".to_vec());
        assert!(!bytes.contains(&0));
    }
}
