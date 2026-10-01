use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::files::replace_file_atomically;
use super::git_url::{
    normalize_git_url, strip_windows_verbatim_owned, validate_git_ref, validate_git_url,
};
use super::manifest::{load_manifest, validate_manifest_for_battery};
use super::path_safety::{
    reject_existing_symlink_ancestors, reject_symlink_components, reject_unsafe_relative_path,
};
use super::sync::verify_synced_checkout;
use super::types::{
    AddBatteryRequest, BatteryAuth, BatteryAuthMethod, BatteryCacheStatus, BatteryInspectResponse,
    BatteryRegistry, BatteryScriptSummary, BatterySummary, InspectBatteryRequest,
    InstalledScriptProvenance, RemoveBatteryRequest, RemoveBatteryResponse,
};
use crate::workspace::Workspace;
use std::fs;
use std::path::{Path, PathBuf};

pub const REGISTRY_VERSION: u32 = 1;

pub fn list_batteries(workspace: &Workspace) -> OperationResult<Vec<BatterySummary>> {
    let paths = BatteryPaths::for_workspace(workspace);
    let registry = read_registry(&paths.registry_path)?;
    Ok(registry.batteries)
}

pub fn inspect_battery(
    workspace: &Workspace,
    request: InspectBatteryRequest,
) -> OperationResult<BatteryInspectResponse> {
    let paths = BatteryPaths::for_workspace(workspace);
    let summary = find_battery(&paths, &request.name)?;
    let cache_status = if summary.resolved_commit.is_some() {
        BatteryCacheStatus::Synced
    } else {
        BatteryCacheStatus::NotSynced
    };
    if matches!(cache_status, BatteryCacheStatus::NotSynced) {
        return Err(OperationError::new(
            OperationErrorCode::NotSynced,
            format!("battery '{}' has not been synced", request.name),
        ));
    }
    let cache_path = cache_path_for_battery(workspace, &summary.name)?;
    let resolved_commit = summary.resolved_commit.as_deref().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::NotSynced,
            format!("battery '{}' has not been synced", request.name),
        )
    })?;
    verify_synced_checkout(&cache_path, resolved_commit)?;
    let manifest = load_manifest(&cache_path)?;
    validate_manifest_for_battery(&cache_path, &manifest, &summary.name)?;
    Ok(BatteryInspectResponse {
        summary,
        manifest,
        cache_status,
    })
}

pub fn list_battery_scripts(
    workspace: &Workspace,
    request: InspectBatteryRequest,
) -> OperationResult<Vec<BatteryScriptSummary>> {
    let response = inspect_battery(workspace, request)?;
    Ok(response
        .manifest
        .scripts
        .into_iter()
        .map(|script| BatteryScriptSummary {
            id: script.id,
            path: script.path,
            description: script.description,
            tags: script.tags,
        })
        .collect())
}

pub fn add_battery(
    workspace: &Workspace,
    request: AddBatteryRequest,
) -> OperationResult<BatterySummary> {
    validate_battery_name(&request.name)?;
    validate_git_url(&request.git_url)?;
    validate_git_ref(&request.requested_ref)?;
    let git_url = normalize_git_url(&request.git_url)?;
    if request.git_url.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url is required",
        ));
    }
    if request.requested_ref.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery ref is required",
        ));
    }

    let paths = BatteryPaths::for_workspace(workspace);
    let mut registry = read_registry(&paths.registry_path)?;
    if registry
        .batteries
        .iter()
        .any(|battery| battery.name == request.name)
    {
        return Err(OperationError::new(
            OperationErrorCode::AlreadyExists,
            format!("battery '{}' already exists", request.name),
        ));
    }
    let cache_abs = paths.cache_path_for(&request.name);
    let cache_path = cache_abs
        .strip_prefix(workspace.root())
        .map(Path::to_path_buf)
        .unwrap_or(cache_abs);
    let auth = match request.token_ref.as_deref() {
        None | Some("") => None,
        Some(raw) => Some(parse_battery_token_ref(raw)?),
    };
    if auth.is_some() && !git_url.to_ascii_lowercase().starts_with("https://") {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "token_ref auth requires an https:// git url",
        ));
    }
    let summary = BatterySummary {
        name: request.name.clone(),
        git_url,
        requested_ref: request.requested_ref,
        resolved_commit: None,
        cache_path,
        last_synced_at: None,
        auth,
    };
    registry.batteries.push(summary.clone());
    write_registry(&paths.registry_path, &registry)?;
    Ok(summary)
}

fn parse_battery_token_ref(raw: &str) -> OperationResult<BatteryAuth> {
    let trimmed = raw.trim();
    let Some(secret_ref) = crate::secrets::SecretRef::parse(trimmed) else {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "token_ref must be a secret://provider/key reference",
        ));
    };
    Ok(BatteryAuth {
        method: BatteryAuthMethod::HttpsTokenRef,
        token_ref: secret_ref.canonical(),
    })
}

pub fn remove_battery(
    workspace: &Workspace,
    request: RemoveBatteryRequest,
) -> OperationResult<RemoveBatteryResponse> {
    let paths = BatteryPaths::for_workspace(workspace);
    let mut registry = read_registry(&paths.registry_path)?;
    validate_battery_name(&request.name)?;
    let index = registry
        .batteries
        .iter()
        .position(|battery| battery.name == request.name)
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotFound,
                format!("battery '{}' was not found", request.name),
            )
        })?;
    let summary = registry.batteries.remove(index);
    let cache_path = cache_path_for_battery(workspace, &summary.name)?;
    let mut cache_removed = false;
    if request.remove_cache && cache_path.exists() {
        fs::remove_dir_all(&cache_path).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to remove battery cache: {err}"),
            )
        })?;
        cache_removed = true;
    }
    write_registry(&paths.registry_path, &registry)?;
    Ok(RemoveBatteryResponse {
        name: request.name,
        cache_removed,
    })
}

fn find_battery(paths: &BatteryPaths, name: &str) -> OperationResult<BatterySummary> {
    validate_battery_name(name)?;
    let registry = read_registry(&paths.registry_path)?;
    registry
        .batteries
        .into_iter()
        .find(|battery| battery.name == name)
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotFound,
                format!("battery '{name}' was not found"),
            )
        })
}

pub(super) fn cache_path_for_battery(
    workspace: &Workspace,
    name: &str,
) -> OperationResult<PathBuf> {
    validate_battery_name(name)?;
    let paths = BatteryPaths::for_workspace(workspace);
    let cache_root = safe_battery_metadata_dir(workspace, &paths.cache_root, "cache")?;
    let path = cache_root.join(name);
    if path.exists() {
        reject_symlink_components(&cache_root, Path::new(name), true)?;
        // `canonicalize` is used only for the containment decision. On
        // Windows it returns a `\\?\` path, while the path handed to Git and
        // the cache filesystem must remain in ordinary DOS form.
        let canonical_root = cache_root.canonicalize().map_err(|err| {
            OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("failed to canonicalize battery cache root: {err}"),
            )
        })?;
        let canonical = path.canonicalize().map_err(|err| {
            OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("failed to canonicalize battery cache path: {err}"),
            )
        })?;
        if !canonical.starts_with(&canonical_root) {
            return Err(OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("battery cache path escapes cache root: {name}"),
            ));
        }
    }
    Ok(path)
}

/// Which battery installed the script at `installed_path`, if any.
///
/// Answered here rather than by the caller reading the provenance files
/// directly, because this module owns that format. A caller that parsed it
/// itself would be coupled to a layout it does not control, and would drift
/// silently the day it changes.
///
/// Only the batteries in `considered` are scanned. That keeps the cost
/// proportional to what a node declared rather than to everything it ever
/// installed, and it means an undeclared battery is not even looked at.
pub fn installing_battery(
    workspace: &Workspace,
    considered: &[String],
    installed_path: &Path,
) -> Option<String> {
    let paths = BatteryPaths::for_workspace(workspace);
    for battery in considered {
        let dir = paths.installed_root.join(sanitize_file_component(battery));
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(contents) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(provenance) = serde_json::from_str::<InstalledScriptProvenance>(&contents)
            else {
                continue;
            };
            if provenance.installed_path == installed_path {
                return Some(provenance.battery_name);
            }
        }
    }
    None
}

#[cfg(unix)]
pub(super) fn installed_root_for_workspace(workspace: &Workspace) -> OperationResult<PathBuf> {
    let paths = BatteryPaths::for_workspace(workspace);
    safe_battery_metadata_dir(workspace, &paths.installed_root, "installed")
}

fn safe_battery_metadata_dir(
    workspace: &Workspace,
    dir: &Path,
    label: &str,
) -> OperationResult<PathBuf> {
    let root = workspace.root().canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize workspace root: {err}"),
        )
    })?;
    let rel = dir.strip_prefix(workspace.root()).map_err(|_| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("battery {label} directory is outside workspace"),
        )
    })?;
    reject_symlink_components(&root, rel, false)?;
    fs::create_dir_all(dir).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create battery {label} directory: {err}"),
        )
    })?;
    reject_symlink_components(&root, rel, true)?;
    let canonical = dir.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize battery {label} directory: {err}"),
        )
    })?;
    let batteries_root = workspace
        .omakure_dir()
        .join("batteries")
        .canonicalize()
        .map_err(|err| {
            OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("failed to canonicalize batteries directory: {err}"),
            )
        })?;
    if !canonical.starts_with(&batteries_root) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("battery {label} directory escapes .omakure/batteries"),
        ));
    }
    // Keep canonical paths for the security comparison above, but never
    // expose Windows' verbatim prefix to Git or cache operations.
    Ok(PathBuf::from(strip_windows_verbatim_owned(
        canonical.to_string_lossy().into_owned(),
    )))
}

pub(super) fn validate_battery_name(name: &str) -> OperationResult<()> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if valid {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery name must be lowercase kebab-case",
        ))
    }
}

pub(super) fn sanitize_file_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryPaths {
    pub registry_path: PathBuf,
    pub cache_root: PathBuf,
    pub installed_root: PathBuf,
}

impl BatteryPaths {
    pub fn for_workspace(workspace: &Workspace) -> Self {
        let batteries_root = workspace.omakure_dir().join("batteries");
        Self {
            registry_path: workspace.omakure_dir().join("batteries.json"),
            cache_root: batteries_root.join("cache"),
            installed_root: batteries_root.join("installed"),
        }
    }

    pub fn cache_path_for(&self, name: &str) -> PathBuf {
        self.cache_root.join(name)
    }
}

pub fn read_registry(path: &Path) -> OperationResult<BatteryRegistry> {
    if !path.exists() {
        return Ok(BatteryRegistry::default());
    }
    let contents = fs::read_to_string(path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to read battery registry: {err}"),
        )
    })?;
    let registry: BatteryRegistry = serde_json::from_str(&contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::RegistryInvalid,
            format!("battery registry is invalid: {err}"),
        )
    })?;
    if registry.version != REGISTRY_VERSION {
        return Err(OperationError::new(
            OperationErrorCode::RegistryInvalid,
            format!("unsupported battery registry version {}", registry.version),
        ));
    }
    for battery in &registry.batteries {
        validate_battery_name(&battery.name)?;
        validate_git_url(&battery.git_url)?;
        validate_git_ref(&battery.requested_ref)?;
        validate_registry_cache_path(&battery.name, &battery.cache_path)?;
    }
    Ok(registry)
}

fn validate_registry_cache_path(name: &str, path: &Path) -> OperationResult<()> {
    reject_unsafe_relative_path(path)?;
    let expected = PathBuf::from(".omakure")
        .join("batteries")
        .join("cache")
        .join(name);
    if path != expected {
        return Err(OperationError::new(
            OperationErrorCode::RegistryInvalid,
            format!("battery cache path must be {}", expected.display()),
        ));
    }
    Ok(())
}

pub fn write_registry(path: &Path, registry: &BatteryRegistry) -> OperationResult<()> {
    if registry.version != REGISTRY_VERSION {
        return Err(OperationError::new(
            OperationErrorCode::RegistryInvalid,
            format!("unsupported battery registry version {}", registry.version),
        ));
    }
    if let Some(parent) = path.parent() {
        reject_existing_symlink_ancestors(parent)?;
        fs::create_dir_all(parent).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to create battery registry directory: {err}"),
            )
        })?;
    }
    let contents = serde_json::to_string_pretty(registry).map_err(|err| {
        OperationError::new(
            OperationErrorCode::RegistryInvalid,
            format!("failed to serialize battery registry: {err}"),
        )
    })?;
    replace_file_atomically(path, contents.as_bytes(), "registry")
}
