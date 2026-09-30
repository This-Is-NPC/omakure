use super::context::NodeContext;
#[cfg(unix)]
use super::fs_unix::{owner_policy, validate_open_file_identity};
#[cfg(windows)]
use super::fs_windows::{validate_open_file_identity, validate_windows_security_handle};
use super::layout::validate_absolute_path;
#[cfg(not(unix))]
use super::security::owner_policy;
use super::security::{
    ensure_safe_parent, sync_directory, validate_file_security_metadata,
    validate_file_security_mode,
};
use super::NodeError;
use crate::util::hex;
use fs2::FileExt;
use rand::rngs::OsRng;
use rand::RngCore;
use std::fs;
#[cfg(test)]
use std::io;
use std::io::Read;
#[cfg(not(windows))]
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicU8, Ordering};

pub(super) const PRIVATE_TOKEN_TOMBSTONE_PREFIX: &str = ".omakure-bootstrap-token-";

pub(crate) const PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT: usize = 10;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateTokenFault {
    None = 0,
    Rename = 1,
    Restore = 2,
    Delete = 3,
}

#[cfg(test)]
static PRIVATE_TOKEN_FAULT: AtomicU8 = AtomicU8::new(PrivateTokenFault::None as u8);

#[cfg(test)]
pub(crate) fn set_private_token_fault(fault: PrivateTokenFault) {
    PRIVATE_TOKEN_FAULT.store(fault as u8, Ordering::SeqCst);
}

#[cfg(test)]
fn private_token_fault(fault: PrivateTokenFault) -> bool {
    PRIVATE_TOKEN_FAULT.load(Ordering::SeqCst) == fault as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivateFileCommitStatus {
    Clean,
    CleanupRequired,
}

pub(crate) struct PrivateTokenLease {
    original_path: PathBuf,
    pub(super) tombstone_path: PathBuf,
    file: fs::File,
    contents: Vec<u8>,
    _lock: Option<PrivateTokenLock>,
}

struct OpenedPrivateFile {
    file: fs::File,
    contents: Vec<u8>,
}

pub(crate) struct PrivateTokenLock {
    _file: fs::File,
}

impl PrivateTokenLease {
    pub(crate) fn contents(&self) -> &[u8] {
        &self.contents
    }

    pub(crate) fn restore(self) -> Result<(), NodeError> {
        #[cfg(test)]
        if private_token_fault(PrivateTokenFault::Restore) {
            return Err(NodeError::Io(io::Error::other(
                "injected bootstrap token restore failure",
            )));
        }
        if fs::symlink_metadata(&self.original_path).is_ok() {
            return Err(NodeError::InsecurePath(
                "bootstrap token path was recreated during enrollment".to_string(),
            ));
        }
        fs::rename(&self.tombstone_path, &self.original_path)?;
        sync_directory(self.original_path.parent().ok_or_else(|| {
            NodeError::UnsafePath("bootstrap token parent is missing".to_string())
        })?)?;
        Ok(())
    }

    pub(crate) fn finish_success(mut self) -> PrivateFileCommitStatus {
        let mut cleanup_required = false;
        #[cfg(not(windows))]
        if self.file.seek(SeekFrom::Start(0)).is_err()
            || self
                .file
                .write_all(&vec![0_u8; self.contents.len()])
                .is_err()
            || self.file.set_len(0).is_err()
            || self.file.sync_all().is_err()
        {
            cleanup_required = true;
        }
        #[cfg(test)]
        let delete_failed = private_token_fault(PrivateTokenFault::Delete);
        #[cfg(not(test))]
        let delete_failed = false;
        drop(self.file);
        let deleted = !delete_failed && fs::remove_file(&self.tombstone_path).is_ok();
        let directory_sync_failed = deleted
            && self
                .tombstone_path
                .parent()
                .map(sync_directory)
                .transpose()
                .is_err();
        cleanup_required |= !deleted || directory_sync_failed;
        if cleanup_required {
            PrivateFileCommitStatus::CleanupRequired
        } else {
            PrivateFileCommitStatus::Clean
        }
    }
}

impl NodeContext {
    pub(crate) fn validate_private_file(&self, path: &Path) -> Result<(), NodeError> {
        validate_file_security_mode(
            path,
            owner_policy(self.platform, self.custom_paths, true)?,
            self.test_mode,
            0o600,
        )
    }

    /// Read a node-local private file without following path reparse points or
    /// allowing an unbounded allocation. The service owner and exact 0600
    /// permissions are part of the file contract.
    pub(crate) fn stage_private_bounded_file(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<PrivateTokenLease, NodeError> {
        let lock = self.acquire_private_token_lock(path)?;
        let file = self.open_private_bounded_file(path, max_bytes)?;
        let parent = path
            .parent()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
        let tombstone_path = private_token_tombstone_path(path)?;
        if fs::symlink_metadata(&tombstone_path).is_ok() {
            return Err(NodeError::InsecurePath(
                "bootstrap token cleanup is pending".to_string(),
            ));
        }
        #[cfg(test)]
        if private_token_fault(PrivateTokenFault::Rename) {
            return Err(NodeError::Io(io::Error::other(
                "injected bootstrap token rename failure",
            )));
        }
        fs::rename(path, &tombstone_path)?;
        if let Err(error) = sync_directory(parent) {
            let _ = fs::rename(&tombstone_path, path);
            let _ = sync_directory(parent);
            return Err(NodeError::Io(error));
        }
        if let Err(error) = validate_open_file_identity(&tombstone_path, &file.file) {
            let _ = fs::rename(&tombstone_path, path);
            let _ = sync_directory(parent);
            return Err(error);
        }
        Ok(PrivateTokenLease {
            original_path: path.to_path_buf(),
            tombstone_path,
            file: file.file,
            contents: file.contents,
            _lock: Some(lock),
        })
    }

    pub(crate) fn acquire_private_token_lock(
        &self,
        path: &Path,
    ) -> Result<PrivateTokenLock, NodeError> {
        validate_absolute_path(self.platform, "private file", path, true)?;
        ensure_safe_parent(path, self.test_mode)?;
        let parent = path
            .parent()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
        let lock_path = parent.join(".omakure-bootstrap-token.lock");
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&lock_path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(NodeError::UnexpectedFileType(
                "bootstrap token lock".to_string(),
            ));
        }
        validate_file_security_metadata(
            &lock_path,
            &metadata,
            owner_policy(self.platform, self.custom_paths, true)?,
            self.test_mode,
            0o600,
        )?;
        file.lock_exclusive()?;
        Ok(PrivateTokenLock { _file: file })
    }

    pub(crate) fn list_private_token_tombstones(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<Vec<PrivateTokenLease>, NodeError> {
        validate_absolute_path(self.platform, "private file", path, true)?;
        ensure_safe_parent(path, self.test_mode)?;
        let parent = path
            .parent()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?
            .to_string_lossy();
        let suffix = format!("-{file_name}");
        let mut tombstones = Vec::new();
        for entry in fs::read_dir(parent)?.take(PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT) {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(PRIVATE_TOKEN_TOMBSTONE_PREFIX) || !name.ends_with(&suffix) {
                continue;
            }
            let tombstone = entry.path();
            let file = self.open_private_bounded_file(&tombstone, max_bytes)?;
            tombstones.push(PrivateTokenLease {
                original_path: path.to_path_buf(),
                tombstone_path: tombstone,
                file: file.file,
                contents: file.contents,
                _lock: None,
            });
        }
        Ok(tombstones)
    }

    fn open_private_bounded_file(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<OpenedPrivateFile, NodeError> {
        validate_absolute_path(self.platform, "private file", path, true)?;
        ensure_safe_parent(path, self.test_mode)?;
        let mut options = crate::util::fs::no_follow_open_options();
        options.read(true);
        #[cfg(not(windows))]
        options.write(true);
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(NodeError::UnexpectedFileType(path.display().to_string()));
        }
        validate_file_security_metadata(
            path,
            &metadata,
            owner_policy(self.platform, self.custom_paths, true)?,
            self.test_mode,
            0o600,
        )?;
        validate_open_file_identity(path, &file)?;
        #[cfg(windows)]
        validate_windows_security_handle(path, &file, self.test_mode)?;
        if metadata.len() > max_bytes as u64 {
            return Err(NodeError::InsecurePath(format!(
                "private file exceeds the {max_bytes}-byte bound"
            )));
        }
        let mut limited = (&file).take(max_bytes as u64 + 1);
        let mut contents = Vec::with_capacity(metadata.len() as usize);
        limited.read_to_end(&mut contents)?;
        if contents.len() > max_bytes {
            return Err(NodeError::InsecurePath(format!(
                "private file exceeds the {max_bytes}-byte bound"
            )));
        }
        Ok(OpenedPrivateFile { file, contents })
    }
}

fn private_token_tombstone_path(path: &Path) -> Result<PathBuf, NodeError> {
    let parent = path
        .parent()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| NodeError::UnsafePath(path.display().to_string()))?
        .to_string_lossy();
    let mut random = [0_u8; 16];
    OsRng.fill_bytes(&mut random);
    let suffix = hex::encode(&random);
    Ok(parent.join(format!(
        "{PRIVATE_TOKEN_TOMBSTONE_PREFIX}{suffix}-{file_name}"
    )))
}
