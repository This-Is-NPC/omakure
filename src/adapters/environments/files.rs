use super::EnvironmentError;
use super::validate_env_key;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn ensure_env_path_safe(
    envs_dir: &Path,
    path: &Path,
    must_exist: bool,
) -> Result<(), EnvironmentError> {
    let envs = envs_dir.canonicalize().map_err(|err| {
        EnvironmentError::ReadFailed(format!(
            "Failed to resolve environments dir {}: {}",
            envs_dir.display(),
            err
        ))
    })?;

    if must_exist && !path.is_file() {
        return Err(EnvironmentError::NotFound {
            name: path.display().to_string(),
        });
    }

    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(EnvironmentError::UnsafePath {
            path: path.display().to_string(),
        });
    }

    let parent = path
        .parent()
        .unwrap_or(envs_dir)
        .canonicalize()
        .map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to resolve environment parent {}: {}",
                path.display(),
                err
            ))
        })?;
    if parent != envs {
        return Err(EnvironmentError::UnsafePath {
            path: path.display().to_string(),
        });
    }

    Ok(())
}

pub(super) fn write_env_params_atomic(
    path: &Path,
    params: &[(&str, &str)],
) -> Result<(), EnvironmentError> {
    let mut contents = String::new();
    for (key, value) in params {
        validate_env_key(key)?;
        if value.contains('\n') || value.contains('\r') {
            return Err(EnvironmentError::WriteFailed(format!(
                "Environment value for {key} must be single-line"
            )));
        }
        contents.push_str(key);
        contents.push('=');
        contents.push_str(value);
        contents.push('\n');
    }
    write_file_atomic(path, contents.as_bytes())
}

pub(super) fn write_active_atomic(path: &Path, name: &str) -> Result<(), EnvironmentError> {
    write_file_atomic(path, format!("{name}\n").as_bytes())
}

pub(super) fn write_file_atomic(path: &Path, contents: &[u8]) -> Result<(), EnvironmentError> {
    let parent = path.parent().ok_or_else(|| EnvironmentError::UnsafePath {
        path: path.display().to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to create environments dir {}: {}",
            parent.display(),
            err
        ))
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("env");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed),
        nonce
    ));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&tmp).map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to write temporary environment file {}: {}",
            tmp.display(),
            err
        ))
    })?;
    file.write_all(contents).map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to write temporary environment file {}: {}",
            tmp.display(),
            err
        ))
    })?;
    file.sync_all().map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to sync temporary environment file {}: {}",
            tmp.display(),
            err
        ))
    })?;
    drop(file);

    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        EnvironmentError::WriteFailed(format!(
            "Failed to replace environment file {}: {}",
            path.display(),
            err
        ))
    })?;
    Ok(())
}
