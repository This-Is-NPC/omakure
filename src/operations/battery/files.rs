use super::super::{OperationError, OperationErrorCode, OperationResult};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(super) fn copy_open_to_file(input: &mut File, output: File) -> OperationResult<()> {
    input.seek(SeekFrom::Start(0)).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to rewind battery script: {err}"),
        )
    })?;
    copy_reader_to_file(input, output)
}

/// Write a staged install's bytes, whatever they are being read from.
///
/// A battery script arrives as an open file in the cache; a baseline arrives
/// as bytes already verified against a signed manifest, with no file to open.
/// The confinement, the temp-then-link dance, and the rollback below are the
/// same either way, and the only thing that differed was where the bytes came
/// from -- which is not a reason for a second copy of any of it.
pub(super) fn copy_reader_to_file(input: &mut dyn Read, mut output: File) -> OperationResult<()> {
    io::copy(input, &mut output).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to copy battery script: {err}"),
        )
    })?;
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to flush install temp file: {err}"),
            )
        })
}

#[cfg(unix)]
pub(super) fn open_existing_file_no_follow(path: &Path) -> OperationResult<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|err| {
            let code = if err.raw_os_error() == Some(libc::ELOOP) {
                OperationErrorCode::UnsafePath
            } else {
                OperationErrorCode::IoFailed
            };
            OperationError::new(code, format!("failed to open battery script: {err}"))
        })?;
    ensure_opened_regular_file(&file)?;
    Ok(file)
}

#[cfg(not(unix))]
pub(super) fn open_existing_file_no_follow(path: &Path) -> OperationResult<File> {
    let file = OpenOptions::new().read(true).open(path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to open battery script: {err}"),
        )
    })?;
    ensure_opened_regular_file(&file)?;
    Ok(file)
}

pub(super) fn ensure_opened_regular_file(file: &File) -> OperationResult<()> {
    let meta = file.metadata().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to inspect battery script: {err}"),
        )
    })?;
    if !meta.is_file() {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            "battery script must be a regular file",
        ));
    }
    Ok(())
}

pub(super) fn replace_file_atomically(
    path: &Path,
    contents: &[u8],
    label: &str,
) -> OperationResult<()> {
    let parent = path.parent().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("{label} path has no parent: {}", path.display()),
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(label);
    for attempt in 0..100u32 {
        let tmp_path = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
        {
            Ok(mut file) => {
                file.write_all(contents)
                    .and_then(|_| file.sync_all())
                    .map_err(|err| {
                        let _ = fs::remove_file(&tmp_path);
                        OperationError::new(
                            OperationErrorCode::IoFailed,
                            format!("failed to write {label} temp file: {err}"),
                        )
                    })?;
                // ReplaceFileW requires the replacement handle to be closed.
                // Keep the temp file's lifetime explicit so repeated atomic
                // writes work on Windows without a sharing violation.
                drop(file);
                #[cfg(windows)]
                {
                    // `rename` cannot replace an existing file on Windows.
                    // ReplaceFileW performs the replacement in one operation,
                    // so readers never observe a remove gap.
                    if path.exists() {
                        crate::util::fs::replace_existing_windows(&tmp_path, path).map_err(
                            |err| {
                                let _ = fs::remove_file(&tmp_path);
                                OperationError::new(
                                    OperationErrorCode::IoFailed,
                                    format!("failed to replace {label}: {err}"),
                                )
                            },
                        )?;
                    } else {
                        fs::rename(&tmp_path, path).map_err(|err| {
                            let _ = fs::remove_file(&tmp_path);
                            OperationError::new(
                                OperationErrorCode::IoFailed,
                                format!("failed to replace {label}: {err}"),
                            )
                        })?;
                    }
                }
                #[cfg(not(windows))]
                fs::rename(&tmp_path, path).map_err(|err| {
                    let _ = fs::remove_file(&tmp_path);
                    OperationError::new(
                        OperationErrorCode::IoFailed,
                        format!("failed to replace {label}: {err}"),
                    )
                })?;
                return Ok(());
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to create {label} temp file: {err}"),
                ));
            }
        }
    }
    Err(OperationError::new(
        OperationErrorCode::Conflict,
        format!("failed to allocate a unique {label} temp file"),
    ))
}
