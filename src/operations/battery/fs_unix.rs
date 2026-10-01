use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::files::ensure_opened_regular_file;
use crate::adapters::fs::unix::{self, FsError};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::Path;

pub(super) fn open_dir_no_follow(path: &Path) -> OperationResult<File> {
    unix::open_dir_no_follow(path).map_err(|error| match error {
        FsError::Nul => OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("path contains NUL byte: {}", path.display()),
        ),
        FsError::Io(error) => OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to open install directory safely: {error}"),
        ),
    })
}

pub(super) fn create_new_file_at(
    parent: &File,
    target_name: &OsStr,
    suffix: &str,
) -> OperationResult<(OsString, File)> {
    let base = target_name.to_string_lossy();
    for attempt in 0..100u32 {
        let name = OsString::from(format!(
            ".{base}.{}.{}.{}",
            std::process::id(),
            attempt,
            suffix
        ));
        match unix::create_new_file_at(parent, &name) {
            Ok(file) => return Ok((name, file)),
            Err(FsError::Nul) => return Err(nul_component_error()),
            Err(FsError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(FsError::Io(error)) => {
                return Err(OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to create install {suffix} file: {error}"),
                ));
            }
        }
    }
    Err(OperationError::new(
        OperationErrorCode::Conflict,
        format!("failed to allocate a unique install {suffix} file"),
    ))
}

pub(super) fn open_existing_file_at_no_follow(
    parent: &File,
    name: &OsStr,
) -> OperationResult<File> {
    let file =
        unix::open_existing_file_at_no_follow(parent, name).map_err(|error| match error {
            FsError::Nul => nul_component_error(),
            FsError::Io(error) => {
                let code = if error.raw_os_error() == Some(libc::ELOOP) {
                    OperationErrorCode::UnsafePath
                } else {
                    OperationErrorCode::IoFailed
                };
                OperationError::new(
                    code,
                    format!("failed to open existing install target: {error}"),
                )
            }
        })?;
    ensure_opened_regular_file(&file)?;
    Ok(file)
}

pub(super) fn renameat_file(parent: &File, from: &OsStr, to: &OsStr) -> OperationResult<()> {
    unix::renameat_file(parent, from, to).map_err(|error| match error {
        FsError::Nul => nul_component_error(),
        FsError::Io(error) => OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to install battery script: {error}"),
        ),
    })
}

pub(super) fn linkat_file(parent: &File, from: &OsStr, to: &OsStr) -> OperationResult<()> {
    unix::linkat_file(parent, from, to).map_err(|error| match error {
        FsError::Nul => nul_component_error(),
        FsError::Io(error) => {
            let code = if error.kind() == io::ErrorKind::AlreadyExists {
                OperationErrorCode::Conflict
            } else {
                OperationErrorCode::IoFailed
            };
            OperationError::new(code, format!("failed to install battery script: {error}"))
        }
    })
}

pub(super) fn unlinkat_file(parent: &File, name: &OsStr) -> OperationResult<()> {
    unix::unlinkat_file(parent, name).map_err(|error| match error {
        FsError::Nul => nul_component_error(),
        FsError::Io(error) => OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to remove install file: {error}"),
        ),
    })
}

fn nul_component_error() -> OperationError {
    OperationError::new(
        OperationErrorCode::UnsafePath,
        "path component contains NUL byte",
    )
}
