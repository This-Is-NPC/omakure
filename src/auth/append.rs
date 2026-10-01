use super::file::{load_tokens_file, parse_tokens_toml, MAX_TOKENS_PER_FILE};
use super::types::AuthError;
use crate::util::entropy;
use crate::util::hex;
use std::fs;
use std::path::{Path, PathBuf};

const APPEND_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn append_token_entry(path: &Path, id: &str, entry: &str) -> Result<(), AuthError> {
    let _lock = AppendLock::acquire(path)?;
    validate_append(path, id)?;
    let staged = staged_token_contents(path, entry)?;
    // Re-parse staged content before replace so we never leave a broken file.
    let _ = parse_tokens_toml(&staged)?;
    // Read under the append lock, so the mode/ownership carried forward is the
    // one belonging to the file this append is actually replacing.
    #[cfg(unix)]
    let replaced_metadata = fs::metadata(path).ok();
    #[cfg(not(unix))]
    let replaced_metadata = None;
    let tmp = write_staged_token_file(path, &staged)?;
    install_staged_token_file(&tmp, path, replaced_metadata.as_ref())?;
    Ok(())
}

fn validate_append(path: &Path, id: &str) -> Result<(), AuthError> {
    // Validate uniqueness before mutating the file.
    if path.exists() {
        let existing = load_tokens_file(path)?;
        if existing.len() >= MAX_TOKENS_PER_FILE {
            return Err(AuthError::Parse(format!(
                "tokens file already has {} entries (max {MAX_TOKENS_PER_FILE})",
                existing.len()
            )));
        }
        if existing.iter().any(|t| t.id == id) {
            return Err(AuthError::DuplicateId(id.to_string()));
        }
    }
    Ok(())
}

fn staged_token_contents(path: &Path, entry: &str) -> Result<String, AuthError> {
    let mut staged = if path.exists() {
        fs::read_to_string(path).map_err(|e| AuthError::Io(e.to_string()))?
    } else {
        "version = 1\n\n".to_string()
    };
    if !staged.ends_with('\n') {
        staged.push('\n');
    }
    if !staged.ends_with("\n\n") && staged.trim_end() != "version = 1" {
        staged.push('\n');
    }
    staged.push_str(entry);
    if !entry.ends_with('\n') {
        staged.push('\n');
    }
    Ok(staged)
}

fn write_staged_token_file(path: &Path, staged: &str) -> Result<PathBuf, AuthError> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    // Randomize the tmp suffix so two concurrent appends in the same process
    // never collide on the path, and so the path is unpredictable (an attacker
    // cannot pre-plant a file/symlink at a guessable tmp name).
    let mut tmp_rand = [0u8; 8];
    entropy::fill_bytes(&mut tmp_rand);
    let tmp_rand: String = hex::encode(&tmp_rand);
    let tmp = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tokens.toml"),
        std::process::id(),
        tmp_rand
    ));
    let mut opts = fs::OpenOptions::new();
    // O_EXCL (`create_new`) so a pre-planted file/symlink at the tmp path is
    // never followed or truncated. Least-privilege 0600 since the tokens file
    // holds credential hashes.
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = match opts.open(&tmp) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            // Stale tmp (e.g. a crashed prior append with the same pid).
            // `remove_file` unlinks the entry itself — it does not follow a
            // symlink target — then O_EXCL re-create refuses any re-plant.
            fs::remove_file(&tmp).map_err(|e| AuthError::Io(e.to_string()))?;
            opts.open(&tmp).map_err(|e| AuthError::Io(e.to_string()))?
        }
        Err(err) => return Err(AuthError::Io(err.to_string())),
    };
    file.write_all(staged.as_bytes())
        .map_err(|e| AuthError::Io(e.to_string()))?;
    file.sync_all().map_err(|e| AuthError::Io(e.to_string()))?;
    Ok(tmp)
}

#[cfg(unix)]
fn install_staged_token_file(
    tmp: &Path,
    destination: &Path,
    replaced_metadata: Option<&fs::Metadata>,
) -> Result<(), AuthError> {
    // Carry the destination's existing mode and ownership onto the replacement.
    // The staged file is deliberately created 0600 and owned by whoever runs
    // the append, but the installed tokens file is `root:omakure 0640` so the
    // unprivileged service user can read it. Renaming a fresh 0600 root-owned
    // file over it would lock the service out of its own credentials.
    if let Some(existing) = replaced_metadata {
        preserve_ownership_and_mode(tmp, existing)?;
    }
    fs::rename(tmp, destination).map_err(|e| AuthError::Io(e.to_string()))
}

#[cfg(windows)]
fn install_staged_token_file(
    tmp: &Path,
    destination: &Path,
    _replaced_metadata: Option<&fs::Metadata>,
) -> Result<(), AuthError> {
    // ReplaceFileW atomically replaces an existing destination while
    // preserving its metadata and security descriptor. Keep the sidecar lock
    // held for the whole operation so token writers remain serialized.
    if destination.exists() {
        crate::util::fs::replace_existing_windows(tmp, destination)
            .map_err(|e| AuthError::Io(e.to_string()))
    } else {
        fs::rename(tmp, destination).map_err(|e| AuthError::Io(e.to_string()))
    }
}

#[cfg(not(any(unix, windows)))]
fn install_staged_token_file(
    tmp: &Path,
    destination: &Path,
    _replaced_metadata: Option<&fs::Metadata>,
) -> Result<(), AuthError> {
    fs::rename(tmp, destination).map_err(|e| AuthError::Io(e.to_string()))
}

/// Apply `existing`'s mode and ownership to the staged replacement at `tmp`.
///
/// Ownership is only changed when it actually differs, so an unprivileged
/// operator appending to a file they already own never needs `CAP_CHOWN`. When
/// it does differ and the chown is refused, that is reported rather than
/// swallowed: silently completing the append is what leaves a node unable to
/// read its own tokens file.
#[cfg(unix)]
fn preserve_ownership_and_mode(tmp: &Path, existing: &fs::Metadata) -> Result<(), AuthError> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let staged = fs::metadata(tmp).map_err(|e| AuthError::Io(e.to_string()))?;
    if staged.uid() != existing.uid() || staged.gid() != existing.gid() {
        let raw = std::ffi::CString::new(tmp.as_os_str().as_bytes())
            .map_err(|e| AuthError::Io(e.to_string()))?;
        // SAFETY: `raw` is a NUL-terminated path we own for the call's duration.
        let rc = unsafe { libc::chown(raw.as_ptr(), existing.uid(), existing.gid()) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(AuthError::Io(format!(
                "could not preserve tokens file ownership {}:{} on {}: {err}",
                existing.uid(),
                existing.gid(),
                tmp.display()
            )));
        }
    }
    fs::set_permissions(tmp, fs::Permissions::from_mode(existing.mode() & 0o7777))
        .map_err(|e| AuthError::Io(e.to_string()))?;
    Ok(())
}

struct AppendLock {
    file: fs::File,
}

fn is_append_lock_contention(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // fs2 can surface these sharing errors directly instead of mapping
        // them to WouldBlock when another process owns the lock.
        matches!(err.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl AppendLock {
    fn acquire(tokens_path: &Path) -> Result<Self, AuthError> {
        let parent = tokens_path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = tokens_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tokens.toml");
        let path = parent.join(format!(".{file_name}.append.lock"));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|err| AuthError::Io(err.to_string()))?;
        let start = std::time::Instant::now();
        loop {
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(err) if is_append_lock_contention(&err) => {
                    if start.elapsed() >= APPEND_LOCK_TIMEOUT {
                        return Err(AuthError::Io(
                            "timed out waiting for another token-file append".to_string(),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(err) => return Err(AuthError::Io(err.to_string())),
            }
        }
    }
}

impl Drop for AppendLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
