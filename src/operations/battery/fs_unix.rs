use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::files::ensure_opened_regular_file;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self};
use std::path::Path;

#[cfg(unix)]
pub(super) fn open_dir_no_follow(path: &Path) -> OperationResult<File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("path contains NUL byte: {}", path.display()),
        )
    })?;
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!(
                "failed to open install directory safely: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
pub(super) fn create_new_file_at(
    parent: &File,
    target_name: &OsStr,
    suffix: &str,
) -> OperationResult<(OsString, File)> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let base = target_name.to_string_lossy();
    for attempt in 0..100u32 {
        let name = OsString::from(format!(
            ".{base}.{}.{}.{}",
            std::process::id(),
            attempt,
            suffix
        ));
        let c_name = cstring_os(&name)?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd >= 0 {
            return Ok((name, unsafe { File::from_raw_fd(fd) }));
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::AlreadyExists {
            continue;
        }
        return Err(OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create install {suffix} file: {err}"),
        ));
    }
    Err(OperationError::new(
        OperationErrorCode::Conflict,
        format!("failed to allocate a unique install {suffix} file"),
    ))
}

#[cfg(unix)]
pub(super) fn open_existing_file_at_no_follow(
    parent: &File,
    name: &OsStr,
) -> OperationResult<File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let c_name = cstring_os(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        let err = io::Error::last_os_error();
        let code = if err.raw_os_error() == Some(libc::ELOOP) {
            OperationErrorCode::UnsafePath
        } else {
            OperationErrorCode::IoFailed
        };
        return Err(OperationError::new(
            code,
            format!("failed to open existing install target: {err}"),
        ));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    ensure_opened_regular_file(&file)?;
    Ok(file)
}

#[cfg(unix)]
pub(super) fn renameat_file(parent: &File, from: &OsStr, to: &OsStr) -> OperationResult<()> {
    use std::os::fd::AsRawFd;

    let from = cstring_os(from)?;
    let to = cstring_os(to)?;
    let rc = unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::IoFailed,
            format!(
                "failed to install battery script: {}",
                io::Error::last_os_error()
            ),
        ))
    }
}

#[cfg(unix)]
pub(super) fn linkat_file(parent: &File, from: &OsStr, to: &OsStr) -> OperationResult<()> {
    use std::os::fd::AsRawFd;

    let from = cstring_os(from)?;
    let to = cstring_os(to)?;
    let rc = unsafe {
        libc::linkat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            0,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        let err = io::Error::last_os_error();
        let code = if err.kind() == io::ErrorKind::AlreadyExists {
            OperationErrorCode::Conflict
        } else {
            OperationErrorCode::IoFailed
        };
        Err(OperationError::new(
            code,
            format!("failed to install battery script: {err}"),
        ))
    }
}

#[cfg(unix)]
pub(super) fn unlinkat_file(parent: &File, name: &OsStr) -> OperationResult<()> {
    use std::os::fd::AsRawFd;

    let name = cstring_os(name)?;
    let rc = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::IoFailed,
            format!(
                "failed to remove install file: {}",
                io::Error::last_os_error()
            ),
        ))
    }
}

#[cfg(unix)]
fn cstring_os(value: &OsStr) -> OperationResult<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(value.as_bytes()).map_err(|_| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            "path component contains NUL byte",
        )
    })
}
