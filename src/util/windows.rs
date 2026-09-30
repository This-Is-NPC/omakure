//! NUL-terminated UTF-16 strings for Win32 `W` APIs.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

pub fn wide_str(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
