use crate::operations::path::has_windows_prefix;
use crate::operations::{OperationError, OperationErrorCode, OperationResult, io_error};
use crate::runtime::script_extensions;
use std::path::{Component, Path, PathBuf};

pub(crate) fn resolve_script_path(script: &str, scripts_root: &Path) -> OperationResult<PathBuf> {
    if script.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "script is required",
        ));
    }

    let root = scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    let normalized_script = script.replace('\\', "/");
    let has_separator = normalized_script.contains('/');
    let path = PathBuf::from(&normalized_script);
    if (!path.is_absolute() && has_windows_prefix(&normalized_script))
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("script path escapes scripts root: {script}"),
        ));
    }
    // Absolute paths are validated after resolution.  Do not compare their
    // spelling with the canonical root here: Windows may present the same
    // path through an 8.3 or verbatim alias (and may normalize its case).
    // `canonical_script_path` performs the containment check on canonical
    // paths once the candidate exists.
    let candidate = if path.is_absolute() {
        path
    } else if has_separator {
        root.join(path)
    } else {
        root.join(script)
    };
    resolve_with_extensions(candidate, &root)
}

fn resolve_with_extensions(path: PathBuf, scripts_root: &Path) -> OperationResult<PathBuf> {
    reject_absolute_path_outside_root(&path, scripts_root)?;
    if path.exists() {
        if path.is_file() {
            return canonical_script_path(&path, scripts_root);
        }
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("script is not a file: {}", path.display()),
        ));
    }
    if path.extension().is_some() {
        return Err(OperationError::new(
            OperationErrorCode::NotFound,
            format!("script not found: {}", path.display()),
        ));
    }
    for ext in script_extensions() {
        let mut candidate = path.clone();
        candidate.set_extension(ext);
        if candidate.is_file() {
            return canonical_script_path(&candidate, scripts_root);
        }
    }
    Err(OperationError::new(
        OperationErrorCode::NotFound,
        format!("script not found: {}", path.display()),
    ))
}

fn reject_absolute_path_outside_root(path: &Path, scripts_root: &Path) -> OperationResult<()> {
    if !path.is_absolute() {
        return Ok(());
    }
    let canonical_root = scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    let mut probe = path;
    loop {
        match probe.canonicalize() {
            Ok(canonical) => {
                if canonical.starts_with(&canonical_root) {
                    return Ok(());
                }
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    format!("script path escapes scripts root: {}", path.display()),
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let Some(parent) = probe.parent() else {
                    return Err(io_error(err));
                };
                if parent == probe {
                    return Err(io_error(err));
                }
                probe = parent;
            }
            Err(err) => return Err(io_error(err)),
        }
    }
}

/// Validate both the requested path and its destination. Metadata (including
/// downloaded but uninstalled Batteries) is never an executable subject.
/// Call again when consuming a queued row, since its path may have changed.
pub(crate) fn canonical_script_path(path: &Path, scripts_root: &Path) -> OperationResult<PathBuf> {
    let canonical = path.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize script path: {err}"),
        )
    })?;
    let canonical_root = scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    let relative = canonical.strip_prefix(&canonical_root).map_err(|_| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("script path escapes scripts root: {}", path.display()),
        )
    })?;
    let requested = path.strip_prefix(scripts_root).unwrap_or(relative);
    if has_reserved_component(relative) || has_reserved_component(requested) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!(
                "script path enters reserved workspace metadata: {}",
                path.display()
            ),
        ));
    }
    if !canonical.is_file() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("script is not a file: {}", path.display()),
        ));
    }
    Ok(canonical)
}

fn has_reserved_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, Component::Normal(name) if name.to_str().is_some_and(|name| {
            crate::workspace::RESERVED_DIR_NAMES.iter().any(|reserved| name.eq_ignore_ascii_case(reserved))
        }))
    })
}
