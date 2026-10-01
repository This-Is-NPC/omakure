#[cfg(unix)]
use super::fs_unix::{mode_is_no_broader_than, UnixOwner};
#[cfg(windows)]
use super::fs_windows::{validate_windows_security, windows_has_reparse_point};
#[cfg(not(unix))]
use super::layout::NodePlatform;
use super::NodeError;
use crate::util::entropy;
use crate::util::hex;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(super) fn ensure_safe_parent(path: &Path, test_mode: bool) -> Result<(), NodeError> {
    if !ensure_safe_parent_if_present(path, test_mode)? {
        return Err(NodeError::UnsafePath(format!(
            "parent does not exist: {}",
            path.parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_else(|| path.display().to_string())
        )));
    }
    Ok(())
}

pub(super) fn ensure_safe_parent_if_present(
    path: &Path,
    test_mode: bool,
) -> Result<bool, NodeError> {
    let parent = path
        .parent()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
    #[cfg(not(windows))]
    let _ = test_mode;
    let mut current = PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(NodeError::Io(error)),
        };
        if metadata.file_type().is_symlink() {
            return Err(NodeError::UnsafePath(current.display().to_string()));
        }
        #[cfg(windows)]
        if !test_mode && windows_has_reparse_point(&current)? {
            return Err(NodeError::UnsafePath(current.display().to_string()));
        }
        if !metadata.file_type().is_dir() {
            return Err(NodeError::UnexpectedFileType(current.display().to_string()));
        }
    }
    Ok(true)
}

pub(crate) fn write_new_file_atomically(
    path: &Path,
    contents: &[u8],
    #[cfg(unix)] mode: u32,
    #[cfg(not(unix))] _mode: u32,
) -> Result<(), NodeError> {
    let parent = path
        .parent()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
    let mut random = [0u8; 8];
    entropy::fill_bytes(&mut random);
    let suffix = hex::encode(&random);
    let temp = parent.join(format!(
        ".{}.tmp-{}-{suffix}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut file = options.open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        // Linking a staged file creates the destination only if it did not
        // appear concurrently; unlike rename, it never clobbers a config.
        fs::hard_link(&temp, path)?;
        fs::remove_file(&temp)?;
        sync_directory(parent)?;
        Ok::<(), io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(NodeError::Io)
}

pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub(super) fn set_directory_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn create_secure_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(not(unix))]
pub(super) fn owner_policy(
    _platform: NodePlatform,
    _custom_paths: bool,
    _state: bool,
) -> Result<(), NodeError> {
    Ok(())
}

pub(super) fn validate_directory_security(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] _owner: (),
    _test_mode: bool,
) -> Result<(), NodeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)?;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(NodeError::InsecurePath(format!(
                "{} must have mode 0700",
                path.display()
            )));
        }
        if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
            return Err(NodeError::InsecurePath(format!(
                "{} has the wrong owner or group",
                path.display()
            )));
        }
    }
    #[cfg(windows)]
    validate_windows_security(path, true, _test_mode)?;
    let _ = path;
    Ok(())
}

pub(super) fn symlink_metadata_if_present(path: &Path) -> Result<Option<fs::Metadata>, NodeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn is_not_found(error: &NodeError) -> bool {
    matches!(error, NodeError::Io(io) if io.kind() == io::ErrorKind::NotFound)
}

pub(super) fn validate_file_security(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] owner: (),
    _test_mode: bool,
) -> Result<(), NodeError> {
    let metadata = fs::symlink_metadata(path)?;
    validate_file_security_metadata(path, &metadata, owner, _test_mode, 0o640)?;
    #[cfg(windows)]
    validate_windows_security(path, false, _test_mode)?;
    Ok(())
}

pub(super) fn validate_file_security_mode(
    path: &Path,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] owner: (),
    _test_mode: bool,
    _expected_mode: u32,
) -> Result<(), NodeError> {
    let metadata = fs::symlink_metadata(path)?;
    validate_file_security_metadata(path, &metadata, owner, _test_mode, _expected_mode)?;
    #[cfg(windows)]
    validate_windows_security(path, false, _test_mode)?;
    Ok(())
}

pub(super) fn validate_file_security_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    #[cfg(unix)] owner: UnixOwner,
    #[cfg(not(unix))] _owner: (),
    _test_mode: bool,
    expected_mode: u32,
) -> Result<(), NodeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode() & 0o777;
        if !mode_is_no_broader_than(mode, expected_mode) {
            return Err(NodeError::InsecurePath(format!(
                "{} has mode {:04o}, which grants more access than the permitted {:04o}; \
                 run: chmod {:o} {}",
                path.display(),
                mode,
                expected_mode,
                expected_mode,
                path.display()
            )));
        }
        if metadata.uid() != owner.uid || metadata.gid() != owner.gid {
            return Err(NodeError::InsecurePath(format!(
                "{} is owned by {}:{} but must be owned by {}:{}; run: chown {}:{} {}",
                path.display(),
                metadata.uid(),
                metadata.gid(),
                owner.uid,
                owner.gid,
                owner.uid,
                owner.gid,
                path.display()
            )));
        }
    }
    let _ = (path, metadata, _test_mode, expected_mode);
    Ok(())
}
