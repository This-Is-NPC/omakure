use super::super::{OperationError, OperationErrorCode, OperationResult};
#[cfg(not(unix))]
use super::files::open_existing_file_no_follow;
#[cfg(unix)]
use super::files::replace_file_atomically;
use super::files::{copy_open_to_file, copy_reader_to_file};
#[cfg(unix)]
use super::fs_unix::{
    create_new_file_at, linkat_file, open_dir_no_follow, open_existing_file_at_no_follow,
    renameat_file, unlinkat_file,
};
use super::git::{run_git_capture, GitCommandSpec};
#[cfg(unix)]
use super::git_url::redacted_git_url;
#[cfg(unix)]
use super::manifest::{open_validated_script_entry, BatteryManifestScript};
#[cfg(unix)]
use super::path_safety::reject_symlink_components;
use super::path_safety::{
    canonical_install_target_path, ensure_install_target_safe, ensure_installed_target_inside,
    reject_reserved_install_path, reject_unsafe_relative_path,
};
#[cfg(unix)]
use super::registry::{
    cache_path_for_battery, inspect_battery, installed_root_for_workspace, sanitize_file_component,
};
#[cfg(unix)]
use super::types::{BatteryInspectResponse, InspectBatteryRequest, InstalledScriptProvenance};
use super::types::{InstallBatteryScriptRequest, InstallBatteryScriptResponse};
use crate::workspace::Workspace;
#[cfg(unix)]
use std::ffi::OsString;
use std::fs;
use std::fs::File;
#[cfg(not(unix))]
use std::fs::OpenOptions;
#[cfg(not(unix))]
use std::io;
use std::io::Read;
#[cfg(unix)]
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub fn install_battery_script(
    workspace: &Workspace,
    request: InstallBatteryScriptRequest,
) -> OperationResult<InstallBatteryScriptResponse> {
    #[cfg(not(unix))]
    {
        let _ = (workspace, request);
        Err(OperationError::new(
            OperationErrorCode::Conflict,
            "battery install is only supported on Unix until non-Unix no-follow install protections are implemented",
        ))
    }
    #[cfg(unix)]
    {
        let inspect = inspect_battery(
            workspace,
            InspectBatteryRequest {
                name: request.battery_name.clone(),
            },
        )?;
        let script = inspect
            .manifest
            .scripts
            .iter()
            .find(|script| script.id == request.script_id)
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::NotFound,
                    format!("battery script '{}' was not found", request.script_id),
                )
            })?;
        let cache_path = cache_path_for_battery(workspace, &inspect.summary.name)?;
        let (_source_path, mut source_file) = open_validated_script_entry(&cache_path, script)?;
        let target = prepare_install_target(workspace, &script.path)?;
        if target.existed && !request.force {
            return Err(OperationError::new(
                OperationErrorCode::Conflict,
                format!(
                    "target script already exists: {}",
                    target.installed_path.display()
                ),
            ));
        }
        let resolved_commit = inspect.summary.resolved_commit.clone().ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotSynced,
                format!("battery '{}' has not been synced", request.battery_name),
            )
        })?;
        let (provenance_path, contents) = prepare_install_provenance(
            workspace,
            &request,
            &inspect,
            script,
            &target.installed_path,
            &resolved_commit,
        )?;
        // The cache entry was already read once for validation, so the reader
        // handed to the install must start at the top of the file again.
        let source_mode = rewind_and_read_source_mode(&mut source_file)?;
        let mut install_state = materialize_install(
            &target.scripts_root,
            &script.path,
            &target.installed_path,
            &target.operation_path,
            InstallSource {
                reader: &mut source_file,
                mode: source_mode,
            },
            request.force,
            target.existed,
        )?;

        if let Err(err) =
            replace_file_atomically(&provenance_path, contents.as_bytes(), "provenance")
        {
            install_state.rollback();
            return Err(err);
        }
        install_state.cleanup();

        Ok(InstallBatteryScriptResponse {
            installed_path: target.installed_path,
            provenance_path,
            battery_name: request.battery_name,
            script_id: request.script_id,
            resolved_commit,
        })
    }
}

#[cfg(unix)]
fn prepare_install_provenance(
    workspace: &Workspace,
    request: &InstallBatteryScriptRequest,
    inspect: &BatteryInspectResponse,
    script: &BatteryManifestScript,
    installed_path: &Path,
    resolved_commit: &str,
) -> OperationResult<(PathBuf, String)> {
    let installed_root = installed_root_for_workspace(workspace)?;
    let provenance_rel =
        PathBuf::from(sanitize_file_component(&request.battery_name)).join(format!(
            "{}.json",
            crate::util::hex::encode(request.script_id.as_bytes())
        ));
    let provenance_path = installed_root.join(&provenance_rel);
    if let Some(parent) = provenance_path.parent() {
        reject_symlink_components(&installed_root, &provenance_rel, false)?;
        fs::create_dir_all(parent).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to create provenance directory: {err}"),
            )
        })?;
        reject_symlink_components(&installed_root, &provenance_rel, false)?;
    }
    let provenance = InstalledScriptProvenance {
        battery_name: request.battery_name.clone(),
        script_id: request.script_id.clone(),
        git_url: redacted_git_url(&inspect.summary.git_url),
        requested_ref: inspect.summary.requested_ref.clone(),
        resolved_commit: resolved_commit.to_string(),
        source_path: script.path.clone(),
        installed_path: installed_path.to_path_buf(),
    };
    let contents = serde_json::to_string_pretty(&provenance).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to serialize install provenance: {err}"),
        )
    })?;
    Ok((provenance_path, contents))
}

struct InstallTarget {
    scripts_root: PathBuf,
    installed_path: PathBuf,
    operation_path: PathBuf,
    existed: bool,
}

fn prepare_install_target(
    workspace: &Workspace,
    relative: &Path,
) -> OperationResult<InstallTarget> {
    reject_unsafe_relative_path(relative)?;
    reject_reserved_install_path(relative)?;
    let installed_path = workspace.scripts_root().join(relative);
    let scripts_root = workspace.scripts_root().canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    if let Some(parent) = installed_path.parent() {
        ensure_install_target_safe(&scripts_root, relative, &installed_path)?;
        fs::create_dir_all(parent).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to create install directory: {err}"),
            )
        })?;
        ensure_install_target_safe(&scripts_root, relative, &installed_path)?;
    }
    let operation_path = canonical_install_target_path(&scripts_root, relative, &installed_path)?;
    let existed = operation_path.exists();
    Ok(InstallTarget {
        scripts_root,
        installed_path,
        operation_path,
        existed,
    })
}

#[cfg(unix)]
fn rewind_and_read_source_mode(source_file: &mut File) -> OperationResult<u32> {
    use std::os::unix::fs::PermissionsExt;

    source_file.seek(SeekFrom::Start(0)).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to rewind battery script: {err}"),
        )
    })?;
    source_file
        .metadata()
        .map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to read battery script mode: {err}"),
            )
        })
        .map(|metadata| metadata.permissions().mode())
}

/// What an installed script may carry of its source's mode.
///
/// Read and execute bits are the source's to give: a script that was
/// executable where it came from stays executable in the workspace, which is
/// what lets it run outside Omakure at all. Write stays with the owner; the
/// workspace is trusted content, and a cache checkout made under a loose
/// umask is not a reason to let the group or the world edit it.
#[cfg(unix)]
pub(crate) const INSTALLED_MODE_MASK: u32 = 0o755;

/// Install one already-verified script into the workspace, force-replacing
/// whatever is there and keeping enough state to put it back.
///
/// The shared install primitive. It lives here rather than in a neutral module
/// because this is where the path-confinement rules were built and where the
/// tests that prove them are: `reject_symlink_components`, the no-follow parent
/// handle, the temp-then-rename dance, and `ensure_installed_target_inside` are
/// a single argument, and splitting the argument from its evidence is how one
/// half of it quietly stops being true.
///
/// The caller supplies bytes it has already decided are correct. This function
/// makes no claim about their provenance — a Battery proves it with a resolved
/// commit, a baseline with a publisher signature over the whole set — and it
/// deliberately cannot: an install primitive that also judged content would
/// give two callers one opinion neither of them wrote.
///
/// Returns the state needed to commit or undo. The caller must call
/// [`InstallState::cleanup`] or [`InstallState::rollback`]; dropping it leaves
/// a backup file behind.
pub(crate) fn install_verified_script(
    workspace: &Workspace,
    relative: &Path,
    bytes: &[u8],
    #[cfg(unix)] mode: u32,
    #[cfg(not(unix))] _mode: u32,
) -> OperationResult<InstallState> {
    let target = prepare_install_target(workspace, relative)?;
    let mut source = bytes;
    materialize_install(
        &target.scripts_root,
        relative,
        &target.installed_path,
        &target.operation_path,
        InstallSource {
            reader: &mut source,
            #[cfg(unix)]
            mode,
        },
        true,
        target.existed,
    )
}

pub(crate) enum InstallState {
    #[cfg(unix)]
    Unix {
        parent: File,
        target_name: OsString,
        backup_name: Option<OsString>,
        target_existed: bool,
    },
    #[cfg(not(unix))]
    Path {
        operation_path: PathBuf,
        backup_path: Option<PathBuf>,
        target_existed: bool,
    },
}

impl InstallState {
    pub(crate) fn rollback(&mut self) {
        match self {
            #[cfg(unix)]
            InstallState::Unix {
                parent,
                target_name,
                backup_name,
                target_existed,
            } => {
                if let Some(backup) = backup_name {
                    let _ = unlinkat_file(parent, target_name);
                    let _ = renameat_file(parent, backup, target_name);
                } else if !*target_existed {
                    let _ = unlinkat_file(parent, target_name);
                }
            }
            #[cfg(not(unix))]
            InstallState::Path {
                operation_path,
                backup_path,
                target_existed,
            } => {
                if let Some(backup) = backup_path {
                    let _ = fs::remove_file(&*operation_path);
                    let _ = fs::rename(backup, &*operation_path);
                } else if !*target_existed {
                    let _ = fs::remove_file(&*operation_path);
                }
            }
        }
    }

    pub(crate) fn cleanup(&mut self) {
        match self {
            #[cfg(unix)]
            InstallState::Unix {
                parent,
                backup_name,
                ..
            } => {
                if let Some(backup) = backup_name.take() {
                    let _ = unlinkat_file(parent, &backup);
                }
            }
            #[cfg(not(unix))]
            InstallState::Path { backup_path, .. } => {
                if let Some(backup) = backup_path.take() {
                    let _ = fs::remove_file(backup);
                }
            }
        }
    }
}

/// The bytes to install and the mode they arrive with.
///
/// A battery script is an open file in the cache and brings its own mode; a
/// baseline script is verified bytes with no file behind them and is given
/// one. The install path does not care which, only that the two travel
/// together.
struct InstallSource<'a> {
    reader: &'a mut dyn Read,
    #[cfg(unix)]
    mode: u32,
}

#[cfg(unix)]
fn materialize_install(
    scripts_root: &Path,
    relative: &Path,
    installed_path: &Path,
    operation_path: &Path,
    source: InstallSource<'_>,
    force: bool,
    target_existed: bool,
) -> OperationResult<InstallState> {
    use std::os::unix::fs::PermissionsExt;

    ensure_install_target_safe(scripts_root, relative, installed_path)?;
    let parent_path = operation_path.parent().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("install target has no parent: {}", operation_path.display()),
        )
    })?;
    let target_name = operation_path.file_name().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!(
                "install target has no file name: {}",
                operation_path.display()
            ),
        )
    })?;
    let parent = open_dir_no_follow(parent_path)?;
    let (tmp_name, tmp_file) = create_new_file_at(&parent, target_name, "tmp")?;
    // On the open descriptor, before the link: a `set_permissions` on the path
    // after close would race ETXTBSY once the target is exec'd, and could land
    // on whatever the name points at by then.
    tmp_file
        .set_permissions(fs::Permissions::from_mode(
            source.mode & INSTALLED_MODE_MASK,
        ))
        .map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to set install mode: {err}"),
            )
        })?;
    copy_reader_to_file(source.reader, tmp_file)?;
    let backup_name = if force && target_existed {
        let (backup_name, backup_file) = create_new_file_at(&parent, target_name, "backup")?;
        let mut input = open_existing_file_at_no_follow(&parent, target_name)?;
        copy_open_to_file(&mut input, backup_file)?;
        Some(backup_name)
    } else {
        None
    };

    let install_result = if force {
        renameat_file(&parent, &tmp_name, target_name)
    } else {
        linkat_file(&parent, &tmp_name, target_name).and_then(|_| unlinkat_file(&parent, &tmp_name))
    };
    if let Err(err) = install_result {
        let _ = unlinkat_file(&parent, &tmp_name);
        if let Some(backup) = &backup_name {
            let _ = renameat_file(&parent, backup, target_name);
        }
        return Err(err);
    }
    ensure_installed_target_inside(scripts_root, operation_path)?;
    Ok(InstallState::Unix {
        parent,
        target_name: target_name.to_os_string(),
        backup_name,
        target_existed,
    })
}

#[cfg(not(unix))]
fn materialize_install(
    scripts_root: &Path,
    relative: &Path,
    installed_path: &Path,
    operation_path: &Path,
    source: InstallSource<'_>,
    force: bool,
    target_existed: bool,
) -> OperationResult<InstallState> {
    let (tmp_path, tmp_file) = unique_install_tmp_file(operation_path)?;
    copy_reader_to_file(source.reader, tmp_file)?;
    ensure_install_target_safe(scripts_root, relative, installed_path)?;
    let backup_path = if force && target_existed {
        Some(backup_existing_target(operation_path)?)
    } else {
        None
    };
    if force {
        ensure_install_target_safe(scripts_root, relative, installed_path)?;
        fs::rename(&tmp_path, operation_path).map_err(|err| {
            let _ = fs::remove_file(&tmp_path);
            if let Some(backup) = &backup_path {
                let _ = fs::rename(backup, operation_path);
            }
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to install battery script: {err}"),
            )
        })?;
    } else {
        ensure_install_target_safe(scripts_root, relative, installed_path)?;
        fs::hard_link(&tmp_path, operation_path).map_err(|err| {
            let _ = fs::remove_file(&tmp_path);
            let code = if err.kind() == io::ErrorKind::AlreadyExists {
                OperationErrorCode::Conflict
            } else {
                OperationErrorCode::IoFailed
            };
            OperationError::new(code, format!("failed to install battery script: {err}"))
        })?;
        fs::remove_file(&tmp_path).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to remove install temp file: {err}"),
            )
        })?;
    }
    ensure_installed_target_inside(scripts_root, operation_path)?;
    Ok(InstallState::Path {
        operation_path: operation_path.to_path_buf(),
        backup_path,
        target_existed,
    })
}

pub(super) fn verify_tracked_blob(cache_path: &Path, relative: &Path) -> OperationResult<()> {
    reject_unsafe_relative_path(relative)?;
    let rel = relative.to_string_lossy().replace('\\', "/");
    let output = run_git_capture(GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-C".into(),
            cache_path.display().to_string(),
            "cat-file".into(),
            "-t".into(),
            format!("HEAD:{rel}"),
        ],
    });
    match output {
        Ok(kind) if kind.trim() == "blob" => Ok(()),
        Err(_) => Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!(
                "battery script is not tracked at HEAD: {}",
                relative.display()
            ),
        )),
        Ok(_) => Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!(
                "battery script is not a tracked file: {}",
                relative.display()
            ),
        )),
    }
}

#[cfg(not(unix))]
fn unique_install_tmp_file(target: &Path) -> OperationResult<(PathBuf, File)> {
    let parent = target.parent().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("install target has no parent: {}", target.display()),
        )
    })?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("script");
    for attempt in 0..100u32 {
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to create install temp file: {err}"),
                ));
            }
        }
    }
    Err(OperationError::new(
        OperationErrorCode::Conflict,
        "failed to allocate a unique install temp file",
    ))
}

#[cfg(not(unix))]
fn backup_existing_target(target: &Path) -> OperationResult<PathBuf> {
    let parent = target.parent().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("install target has no parent: {}", target.display()),
        )
    })?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("script");
    for attempt in 0..100u32 {
        let backup = parent.join(format!(
            ".{file_name}.{}.{}.backup",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(file) => {
                let mut input = match open_existing_file_no_follow(target) {
                    Ok(input) => input,
                    Err(err) => {
                        let _ = fs::remove_file(&backup);
                        return Err(err);
                    }
                };
                if let Err(err) = copy_open_to_file(&mut input, file) {
                    let _ = fs::remove_file(&backup);
                    return Err(err);
                }
                return Ok(backup);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to create install backup: {err}"),
                ));
            }
        }
    }
    Err(OperationError::new(
        OperationErrorCode::Conflict,
        "failed to allocate a unique install backup file",
    ))
}
