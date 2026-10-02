use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::git_url::strip_windows_verbatim_owned;
use std::fs;
use std::io::{self};
use std::path::{Component, Path, PathBuf};

pub fn reject_unsafe_relative_path(path: &Path) -> OperationResult<()> {
    if path.is_absolute() {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("absolute battery path is not allowed: {}", path.display()),
        ));
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    format!("unsafe battery path is not allowed: {}", path.display()),
                ));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

pub(super) fn reject_reserved_install_path(path: &Path) -> OperationResult<()> {
    let first = path.components().find_map(|component| match component {
        Component::Normal(part) => part.to_str(),
        _ => None,
    });
    if first.is_some_and(|part| part.starts_with('.')) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!(
                "battery install path targets reserved metadata: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(super) fn ensure_install_target_safe(
    scripts_root: &Path,
    relative: &Path,
    installed_path: &Path,
) -> OperationResult<()> {
    reject_symlink_components(scripts_root, relative, false)?;
    let parent = installed_path.parent().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("install target has no parent: {}", installed_path.display()),
        )
    })?;
    if parent.exists() {
        let parent = parent.canonicalize().map_err(|err| {
            OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("failed to canonicalize install directory: {err}"),
            )
        })?;
        if !parent.starts_with(scripts_root) {
            return Err(OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("install path escapes scripts root: {}", relative.display()),
            ));
        }
    }
    Ok(())
}

pub(super) fn canonical_install_target_path(
    scripts_root: &Path,
    relative: &Path,
    installed_path: &Path,
) -> OperationResult<PathBuf> {
    ensure_install_target_safe(scripts_root, relative, installed_path)?;
    let parent = installed_path.parent().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("install target has no parent: {}", installed_path.display()),
        )
    })?;
    let parent = parent.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize install directory: {err}"),
        )
    })?;
    if !parent.starts_with(scripts_root) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("install path escapes scripts root: {}", relative.display()),
        ));
    }
    let name = installed_path.file_name().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!(
                "install target has no file name: {}",
                installed_path.display()
            ),
        )
    })?;
    Ok(parent.join(name))
}

pub(super) fn ensure_installed_target_inside(
    scripts_root: &Path,
    operation_path: &Path,
) -> OperationResult<()> {
    let canonical = operation_path.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize installed script: {err}"),
        )
    })?;
    if !canonical.starts_with(scripts_root) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!(
                "installed script escaped scripts root: {}",
                operation_path.display()
            ),
        ));
    }
    Ok(())
}

pub fn confined_existing_path(root: &Path, relative: &Path) -> OperationResult<PathBuf> {
    reject_unsafe_relative_path(relative)?;
    let root = root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize battery cache root: {err}"),
        )
    })?;
    let full = root.join(relative).canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize battery path: {err}"),
        )
    })?;
    if !full.starts_with(&root) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("battery path escapes cache root: {}", relative.display()),
        ));
    }
    Ok(PathBuf::from(strip_windows_verbatim_owned(
        full.to_string_lossy().into_owned(),
    )))
}

pub(super) fn reject_symlink_components(
    root: &Path,
    relative: &Path,
    target_must_exist: bool,
) -> OperationResult<()> {
    reject_unsafe_relative_path(relative)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    format!("battery symlink is not allowed: {}", current.display()),
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound && !target_must_exist => break,
            Err(err) => {
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    format!("failed to inspect battery path component: {err}"),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn reject_existing_symlink_ancestors(path: &Path) -> OperationResult<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    format!(
                        "symlinked metadata path is not allowed: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => break,
            Err(err) => {
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    format!("failed to inspect metadata path component: {err}"),
                ));
            }
        }
    }
    Ok(())
}
