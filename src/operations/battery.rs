use crate::domain::{extract_schema_block, parse_schema};
use crate::runtime::{script_kind, ScriptKind};
use crate::secrets::{self, SecretAccess};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use super::{OperationError, OperationErrorCode, OperationResult};

pub const REGISTRY_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "omakure-battery.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryRef {
    pub name: String,
}

/// How a Battery authenticates to a private HTTPS remote.
///
/// Registry stores method + secret ref only — never resolved plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryAuthMethod {
    HttpsTokenRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryAuth {
    pub method: BatteryAuthMethod,
    /// Canonical `secret://provider/key` ref. Never a plaintext token.
    pub token_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatterySummary {
    pub name: String,
    pub git_url: String,
    pub requested_ref: String,
    pub resolved_commit: Option<String>,
    pub cache_path: PathBuf,
    pub last_synced_at: Option<String>,
    /// Present when the Battery uses private HTTPS auth via a secret ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<BatteryAuth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryRegistry {
    pub version: u32,
    pub batteries: Vec<BatterySummary>,
}

impl Default for BatteryRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            batteries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryScriptSummary {
    pub id: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectBatteryRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryInspectResponse {
    pub summary: BatterySummary,
    pub manifest: BatteryManifest,
    pub cache_status: BatteryCacheStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryCacheStatus {
    NotSynced,
    Synced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddBatteryRequest {
    pub name: String,
    pub git_url: String,
    pub requested_ref: String,
    /// Optional `secret://…` ref for private HTTPS clone/fetch (GIT_ASKPASS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallBatteryScriptRequest {
    pub battery_name: String,
    pub script_id: String,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncBatteryRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallBatteryScriptResponse {
    pub installed_path: PathBuf,
    pub provenance_path: PathBuf,
    pub battery_name: String,
    pub script_id: String,
    pub resolved_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveBatteryResponse {
    pub name: String,
    pub cache_removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InstalledScriptProvenance {
    battery_name: String,
    script_id: String,
    git_url: String,
    requested_ref: String,
    resolved_commit: String,
    source_path: PathBuf,
    installed_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveBatteryRequest {
    pub name: String,
    pub remove_cache: bool,
}

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

pub fn sync_battery(
    workspace: &Workspace,
    request: SyncBatteryRequest,
) -> OperationResult<BatterySummary> {
    sync_battery_with_access(
        workspace,
        request,
        GitTransportPolicy::Default,
        &SecretAccess::allow_all(),
    )
}

pub fn sync_battery_https_only(
    workspace: &Workspace,
    request: SyncBatteryRequest,
) -> OperationResult<BatterySummary> {
    sync_battery_with_access(
        workspace,
        request,
        GitTransportPolicy::HttpsOnly,
        &SecretAccess::allow_all(),
    )
}

/// Sync with an explicit secret ACL (HTTP uses this after `credentials:use`).
pub fn sync_battery_https_only_with_access(
    workspace: &Workspace,
    request: SyncBatteryRequest,
    access: &SecretAccess,
) -> OperationResult<BatterySummary> {
    sync_battery_with_access(workspace, request, GitTransportPolicy::HttpsOnly, access)
}

fn sync_battery_with_access(
    workspace: &Workspace,
    request: SyncBatteryRequest,
    policy: GitTransportPolicy,
    access: &SecretAccess,
) -> OperationResult<BatterySummary> {
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
    validate_git_url(&registry.batteries[index].git_url)?;
    validate_git_ref(&registry.batteries[index].requested_ref)?;
    let http_pin = resolve_public_git_endpoint(&registry.batteries[index].git_url)?;
    let auth = registry.batteries[index].auth.clone();
    let askpass = prepare_git_askpass(workspace, auth.as_ref(), access)?;
    let git_ctx = GitExecContext {
        policy,
        askpass: askpass.as_ref(),
        http_pin: http_pin.as_ref(),
    };
    if http_pin.is_some() {
        assert_git_http_pinning_supported(&git_ctx)?;
    }
    let cache_path = cache_path_for_battery(workspace, &registry.batteries[index].name)?;
    let sync_result = (|| -> OperationResult<BatterySummary> {
        if !cache_path.join(".git").is_dir() {
            if cache_path.exists() {
                fs::remove_dir_all(&cache_path).map_err(|err| {
                    OperationError::new(
                        OperationErrorCode::IoFailed,
                        format!("failed to clear invalid battery cache: {err}"),
                    )
                })?;
            }
            run_git_with_context(
                git_clone_spec(&registry.batteries[index].git_url, &cache_path),
                &git_ctx,
            )?;
            reject_unsafe_local_git_config(&cache_path)?;
        } else {
            verify_cache_origin_with_context(
                &cache_path,
                &registry.batteries[index].git_url,
                &git_ctx,
            )?;
        }
        run_git_with_context(
            git_fetch_spec(&cache_path, &registry.batteries[index].requested_ref),
            &git_ctx,
        )?;
        let fetched_commit = run_git_capture_with_context(
            GitCommandSpec {
                program: "git".into(),
                args: vec![
                    "-C".into(),
                    cache_path.display().to_string(),
                    "rev-parse".into(),
                    "FETCH_HEAD^{commit}".into(),
                ],
            },
            &git_ctx,
        )?;
        run_git_with_context(
            git_checkout_detached_spec(&cache_path, fetched_commit.trim()),
            &git_ctx,
        )?;
        let resolved_commit = run_git_capture_with_context(
            GitCommandSpec {
                program: "git".into(),
                args: vec![
                    "-C".into(),
                    cache_path.display().to_string(),
                    "rev-parse".into(),
                    "HEAD".into(),
                ],
            },
            &git_ctx,
        )?
        .trim()
        .to_string();
        let manifest = load_manifest(&cache_path)?;
        validate_manifest_for_battery(&cache_path, &manifest, &registry.batteries[index].name)?;
        registry.batteries[index].resolved_commit = Some(resolved_commit);
        registry.batteries[index].last_synced_at = Some(chrono::Utc::now().to_rfc3339());
        let summary = registry.batteries[index].clone();
        write_registry(&paths.registry_path, &registry)?;
        // Ensure plaintext never lands in the registry file.
        if let Some(auth) = &summary.auth {
            let registry_text = fs::read_to_string(&paths.registry_path).unwrap_or_default();
            if let Some(token) = askpass.as_ref().map(|a| a.token.as_str()) {
                if !token.is_empty() && registry_text.contains(token) {
                    return Err(OperationError::new(
                        OperationErrorCode::Conflict,
                        "refusing to persist resolved battery credentials",
                    ));
                }
            }
            let _ = auth;
        }
        Ok(summary)
    })();
    drop(askpass);
    sync_result
}

struct GitAskpassGuard {
    dir: PathBuf,
    script_path: PathBuf,
    token: String,
}

impl Drop for GitAskpassGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.script_path);
        let token_path = self.dir.join("token");
        let _ = fs::remove_file(&token_path);
        let _ = fs::remove_dir(&self.dir);
    }
}

struct GitExecContext<'a> {
    policy: GitTransportPolicy,
    askpass: Option<&'a GitAskpassGuard>,
    http_pin: Option<&'a GitHttpPin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHttpPin {
    host: String,
    port: u16,
    address: std::net::IpAddr,
    credential_authority: String,
}

impl GitHttpPin {
    fn curlopt_resolve(&self) -> String {
        // curl's `HOST:PORT:ADDRESS` --resolve syntax needs `[HOST]` whenever
        // HOST itself is an IPv6 literal, or the colons make it ambiguous
        // with the PORT/ADDRESS delimiters.
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let address = match self.address {
            std::net::IpAddr::V4(address) => address.to_string(),
            std::net::IpAddr::V6(address) => format!("[{address}]"),
        };
        format!("{host}:{}:{address}", self.port)
    }

    fn credential_authority(&self) -> &str {
        &self.credential_authority
    }
}

fn prepare_git_askpass(
    workspace: &Workspace,
    auth: Option<&BatteryAuth>,
    access: &SecretAccess,
) -> OperationResult<Option<GitAskpassGuard>> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    if !matches!(auth.method, BatteryAuthMethod::HttpsTokenRef) {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "unsupported battery auth method",
        ));
    }
    let token = resolve_battery_token(workspace, &auth.token_ref, access)?;
    if token.is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "battery token_ref resolved to an empty secret",
        ));
    }
    // Unique per-sync directory so concurrent Battery ops never share token files.
    let tmp_root = workspace.omakure_dir().join("tmp");
    fs::create_dir_all(&tmp_root).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create askpass temp root: {err}"),
        )
    })?;
    let dir = {
        use rand::RngCore;
        let mut last_err = None;
        let mut created = None;
        for _ in 0..8 {
            let mut bytes = [0u8; 8];
            rand::thread_rng().fill_bytes(&mut bytes);
            let candidate = tmp_root.join(format!(
                "git-askpass-{}-{}",
                std::process::id(),
                u64::from_le_bytes(bytes)
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    created = Some(candidate);
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_err = Some(err);
                    continue;
                }
                Err(err) => {
                    return Err(OperationError::new(
                        OperationErrorCode::IoFailed,
                        format!("failed to create askpass directory: {err}"),
                    ));
                }
            }
        }
        created.ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!(
                    "failed to allocate unique askpass directory: {}",
                    last_err
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "exhausted retries".into())
                ),
            )
        })?
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dir)
            .map_err(|err| {
                OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to read askpass directory metadata: {err}"),
                )
            })?
            .permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&dir, perms).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to set askpass directory permissions: {err}"),
            )
        })?;
    }
    let token_path = dir.join("token");
    write_secret_file(&token_path, token.as_bytes())?;
    let script_path = dir.join("askpass.sh");
    // Resolve token via relative path under $0's directory — no shell-quoted
    // absolute paths (avoids `'` injection and path-with-spaces breakage).
    let script = "#!/bin/sh\nDIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\n[ -n \"$OMAKURE_GIT_AUTHORITY\" ] || exit 1\ncase \"$1\" in\n*\"//$OMAKURE_GIT_AUTHORITY/\"*|*\"//$OMAKURE_GIT_AUTHORITY'\"*|*\"@$OMAKURE_GIT_AUTHORITY/\"*|*\"@$OMAKURE_GIT_AUTHORITY'\"*) ;;\n*) exit 1 ;;\nesac\ncase \"$1\" in\n*Username*|*username*) printf '%s\\n' 'x-access-token' ;;\n*) cat \"$DIR/token\" ;;\nesac\n";
    write_secret_file(&script_path, script.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)
            .map_err(|err| {
                OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to read askpass script metadata: {err}"),
                )
            })?
            .permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&script_path, perms).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to set askpass script permissions: {err}"),
            )
        })?;
    }
    Ok(Some(GitAskpassGuard {
        dir,
        script_path,
        token,
    }))
}

fn write_secret_file(path: &Path, contents: &[u8]) -> OperationResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create secret file: {err}"),
        )
    })?;
    file.write_all(contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to write secret file: {err}"),
        )
    })?;
    file.sync_all().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to sync secret file: {err}"),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|err| {
                OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to read secret file metadata: {err}"),
                )
            })?
            .permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to set secret file permissions: {err}"),
            )
        })?;
    }
    Ok(())
}

fn resolve_battery_token(
    workspace: &Workspace,
    token_ref: &str,
    access: &SecretAccess,
) -> OperationResult<String> {
    secrets::resolve_secret_value(workspace, token_ref, access).map_err(|err| {
        OperationError::new(
            OperationErrorCode::Forbidden,
            format!("failed to resolve battery token_ref: {err}"),
        )
    })
}

pub fn install_battery_script(
    workspace: &Workspace,
    request: InstallBatteryScriptRequest,
) -> OperationResult<InstallBatteryScriptResponse> {
    #[cfg(not(unix))]
    {
        let _ = (workspace, request);
        return Err(OperationError::new(
            OperationErrorCode::Conflict,
            "battery install is only supported on Unix until non-Unix no-follow install protections are implemented",
        ));
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
        reject_unsafe_relative_path(&script.path)?;
        reject_reserved_install_path(&script.path)?;
        let installed_path = workspace.scripts_root().join(&script.path);
        let scripts_root = workspace.scripts_root().canonicalize().map_err(|err| {
            OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("failed to canonicalize scripts root: {err}"),
            )
        })?;
        if let Some(parent) = installed_path.parent() {
            ensure_install_target_safe(&scripts_root, &script.path, &installed_path)?;
            fs::create_dir_all(parent).map_err(|err| {
                OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to create install directory: {err}"),
                )
            })?;
            ensure_install_target_safe(&scripts_root, &script.path, &installed_path)?;
        }
        let operation_path =
            canonical_install_target_path(&scripts_root, &script.path, &installed_path)?;
        let target_existed = operation_path.exists();
        if target_existed && !request.force {
            return Err(OperationError::new(
                OperationErrorCode::Conflict,
                format!("target script already exists: {}", installed_path.display()),
            ));
        }
        let resolved_commit = inspect.summary.resolved_commit.clone().ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotSynced,
                format!("battery '{}' has not been synced", request.battery_name),
            )
        })?;
        let installed_root = installed_root_for_workspace(workspace)?;
        let provenance_rel = PathBuf::from(sanitize_file_component(&request.battery_name))
            .join(format!("{}.json", hex_encode(request.script_id.as_bytes())));
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
            resolved_commit: resolved_commit.clone(),
            source_path: script.path.clone(),
            installed_path: installed_path.clone(),
        };
        let contents = serde_json::to_string_pretty(&provenance).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to serialize install provenance: {err}"),
            )
        })?;
        let mut install_state = materialize_install(
            &scripts_root,
            &script.path,
            &installed_path,
            &operation_path,
            &mut source_file,
            request.force,
            target_existed,
        )?;

        if let Err(err) = write_atomic(&provenance_path, contents.as_bytes(), "provenance") {
            install_state.rollback();
            return Err(err);
        }
        install_state.cleanup();

        Ok(InstallBatteryScriptResponse {
            installed_path,
            provenance_path,
            battery_name: request.battery_name,
            script_id: request.script_id,
            resolved_commit,
        })
    }
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

fn cache_path_for_battery(workspace: &Workspace, name: &str) -> OperationResult<PathBuf> {
    validate_battery_name(name)?;
    let paths = BatteryPaths::for_workspace(workspace);
    let cache_root = safe_battery_metadata_dir(workspace, &paths.cache_root, "cache")?;
    let path = cache_root.join(name);
    if path.exists() {
        reject_symlink_components(&cache_root, Path::new(name), true)?;
        let canonical = path.canonicalize().map_err(|err| {
            OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("failed to canonicalize battery cache path: {err}"),
            )
        })?;
        if !canonical.starts_with(&cache_root) {
            return Err(OperationError::new(
                OperationErrorCode::UnsafePath,
                format!("battery cache path escapes cache root: {name}"),
            ));
        }
    }
    Ok(path)
}

fn installed_root_for_workspace(workspace: &Workspace) -> OperationResult<PathBuf> {
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
    Ok(canonical)
}

fn validate_battery_name(name: &str) -> OperationResult<()> {
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

fn validate_git_url(value: &str) -> OperationResult<()> {
    if value.trim().is_empty()
        || value.starts_with('-')
        || value.chars().any(char::is_control)
        || value.contains('?')
        || value.contains('#')
    {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url is invalid",
        ));
    }
    if url_contains_credentials(value) {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url must not contain credentials",
        ));
    }
    Ok(())
}

/// Registration-time SSRF guard: reject Battery HTTP(S) sources whose host is a
/// **literal** private, loopback, link-local, or cloud-metadata IP. Purely
/// syntactic — no DNS — so it stays hermetic and catches the obvious
/// `https://169.254.169.254`, `https://10.0.0.5`, `https://[::1]` cases without
/// pinning registration to name resolution. The resolving guard runs at fetch
/// time (see [`assert_public_git_host`]). Non-network schemes are skipped.
pub fn assert_git_url_host_public_literal(git_url: &str) -> OperationResult<()> {
    let trimmed = git_url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Ok(());
    }
    let host = git_url_host(trimmed).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url has no host",
        )
    })?;
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip_is_private(ip) {
            return Err(private_git_host_error());
        }
    }
    Ok(())
}

/// Fetch-time SSRF guard used by HTTP policy checks. Battery sync additionally
/// pins Git/curl to the verified address so Git cannot perform a second DNS
/// lookup after this check.
pub fn assert_public_git_host(git_url: &str) -> OperationResult<()> {
    resolve_public_git_endpoint(git_url).map(|_| ())
}

fn resolve_public_git_endpoint(git_url: &str) -> OperationResult<Option<GitHttpPin>> {
    use std::net::ToSocketAddrs;

    let trimmed = git_url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Ok(None);
    }
    let (host, port) = git_url_endpoint(trimmed).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url has no host",
        )
    })?;
    let credential_authority = git_url_authority(trimmed).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url has no authority",
        )
    })?;

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip_is_private(ip) {
            return Err(private_git_host_error());
        }
        return Ok(Some(GitHttpPin {
            host,
            port,
            address: ip,
            credential_authority,
        }));
    }

    let resolved = (host.as_str(), port).to_socket_addrs().map_err(|err| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("battery git host did not resolve: {err}"),
        )
    })?;
    let mut pinned = None;
    for addr in resolved {
        if ip_is_private(addr.ip()) {
            return Err(private_git_host_error());
        }
        pinned.get_or_insert(addr.ip());
    }
    let address = pinned.ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git host did not resolve to any address",
        )
    })?;
    Ok(Some(GitHttpPin {
        host,
        port,
        address,
        credential_authority,
    }))
}

fn private_git_host_error() -> OperationError {
    OperationError::new(
        OperationErrorCode::Forbidden,
        "battery git url resolves to a private, loopback, or link-local address; refused to prevent SSRF",
    )
}

/// Extract the bare host from an `scheme://` URL, stripping userinfo and port
/// and unwrapping `[ipv6]` literals.
fn git_url_host(url: &str) -> Option<String> {
    git_url_endpoint(url).map(|(host, _)| host)
}

fn git_url_endpoint(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(after_bracket) = host_port.strip_prefix('[') {
        let (host, suffix) = after_bracket.split_once(']')?;
        let port = match suffix.strip_prefix(':') {
            Some(value) => value.parse().ok()?,
            None if suffix.is_empty() => default_port,
            None => return None,
        };
        return (!host.is_empty()).then(|| (host.to_string(), port));
    }
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (host_port, default_port),
    };
    (!host.is_empty()).then(|| (host.to_string(), port))
}

fn git_url_authority(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    (!authority.is_empty()).then(|| authority.to_string())
}

/// Whether an address is in a private / non-routable / metadata range that a
/// remote caller must never be able to make the engine reach.
fn ip_is_private(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => ipv4_is_private(v4),
        IpAddr::V6(v6) => ipv6_is_private(v6),
    }
}

fn ipv4_is_private(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || o[0] == 0
        || is_carrier_grade_nat(o)
        || is_ietf_protocol_assignment_or_relay_anycast(o)
        || is_benchmarking_range(o)
        // Multicast and reserved/future-use space are never public unicast.
        || o[0] >= 224
        // Azure platform virtual IP is reachable only from tenant networks.
        || o == [168, 63, 129, 16]
}

/// Carrier-grade NAT `100.64.0.0/10`.
fn is_carrier_grade_nat(o: [u8; 4]) -> bool {
    o[0] == 100 && (o[1] & 0xc0) == 64
}

/// IETF protocol assignments (`192.0.0.0/24`) and the deprecated 6to4 relay
/// anycast prefix (`192.88.99.0/24`).
fn is_ietf_protocol_assignment_or_relay_anycast(o: [u8; 4]) -> bool {
    (o[0] == 192 && o[1] == 0 && o[2] == 0) || (o[0] == 192 && o[1] == 88 && o[2] == 99)
}

/// Benchmarking networks (`198.18.0.0/15`) are commonly routed inside
/// infrastructure.
fn is_benchmarking_range(o: [u8; 4]) -> bool {
    o[0] == 198 && matches!(o[1], 18 | 19)
}

fn ipv6_is_private(v6: std::net::Ipv6Addr) -> bool {
    if let Some(mapped) = v6.to_ipv4_mapped() {
        return ipv4_is_private(mapped);
    }
    let seg = v6.segments();
    // Several IPv6 forms embed an IPv4 address that a transition mechanism
    // will actually route to. `to_ipv4_mapped` only unwraps `::ffff:a.b.c.d`,
    // so classify the rest by their embedded IPv4 — otherwise e.g.
    // `[2002:0a00:0005::]` (6to4 → 10.0.0.5) or `[::7f00:1]` (127.0.0.1)
    // would slip through as "public".
    if let Some(embedded) = ipv6_embedded_ipv4(seg) {
        return ipv4_is_private(embedded);
    }
    // NAT64 local-use prefix `64:ff9b:1::/48` (RFC 8215) is local-use only and
    // never names a legitimate public host, so block the whole prefix rather
    // than trying to decode every RFC 6052 embedding.
    if is_nat64_local_use(seg) {
        return true;
    }
    ipv6_is_reserved_or_non_global(v6, seg)
}

fn ipv4_embedded_in_ipv6(hi: u16, lo: u16) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::new(
        (hi >> 8) as u8,
        (hi & 0xff) as u8,
        (lo >> 8) as u8,
        (lo & 0xff) as u8,
    )
}

/// Extract the embedded IPv4 for the transition mechanisms that carry a
/// routable address: IPv4-compatible, NAT64 WKP, 6to4, and Teredo.
fn ipv6_embedded_ipv4(seg: [u16; 8]) -> Option<std::net::Ipv4Addr> {
    if is_ipv4_compatible(seg) || is_nat64_wkp(seg) {
        return Some(ipv4_embedded_in_ipv6(seg[6], seg[7]));
    }
    if is_6to4(seg) {
        return Some(ipv4_embedded_in_ipv6(seg[1], seg[2]));
    }
    if is_teredo(seg) {
        return Some(ipv4_embedded_in_ipv6(!seg[6], !seg[7]));
    }
    None
}

/// IPv4-compatible `::a.b.c.d` (deprecated): embedded IPv4 in the low 32
/// bits, with the all-zero and loopback (`::1`) addresses excluded.
fn is_ipv4_compatible(seg: [u16; 8]) -> bool {
    seg[..6].iter().all(|s| *s == 0) && (seg[6] != 0 || seg[7] > 1)
}

/// NAT64 Well-Known Prefix `64:ff9b::/96`: embedded IPv4 in the low 32 bits.
fn is_nat64_wkp(seg: [u16; 8]) -> bool {
    seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0)
}

/// 6to4 `2002::/16`: embedded IPv4 (the 6to4 gateway) in seg[1..3].
fn is_6to4(seg: [u16; 8]) -> bool {
    seg[0] == 0x2002
}

/// Teredo `2001:0000::/32`: client IPv4 is the bitwise complement of the low
/// 32 bits.
fn is_teredo(seg: [u16; 8]) -> bool {
    seg[0] == 0x2001 && seg[1] == 0x0000
}

fn is_nat64_local_use(seg: [u16; 8]) -> bool {
    seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2] == 0x0001
}

fn ipv6_is_reserved_or_non_global(v6: std::net::Ipv6Addr, seg: [u16; 8]) -> bool {
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        // Only global-unicast 2000::/3 is accepted after transition forms.
        || (seg[0] & 0xe000) != 0x2000
        // Documentation, benchmarking, and ORCHID are not public endpoints.
        || (seg[0] == 0x2001 && seg[1] == 0x0db8)
        || (seg[0] == 0x2001 && seg[1] == 0x0002 && seg[2] == 0)
        || (seg[0] == 0x2001 && (seg[1] & 0xfff0) == 0x0010)
        || (seg[0] == 0x2001 && (seg[1] & 0xfff0) == 0x0020)
        // Unique-local fc00::/7
        || (seg[0] & 0xfe00) == 0xfc00
        // Link-local fe80::/10
        || (seg[0] & 0xffc0) == 0xfe80
}

/// Reject local / file Battery sources when deploy policy disallows them.
pub fn assert_local_battery_allowed(allow_local: bool, git_url: &str) -> OperationResult<()> {
    if allow_local {
        return Ok(());
    }
    let lower = git_url.trim().to_ascii_lowercase();
    let is_local = lower.starts_with("file://")
        || Path::new(git_url.trim()).is_absolute()
        || (!lower.contains("://") && !lower.contains('@'));
    if is_local {
        return Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy sources.allow_local_batteries=false",
        ));
    }
    Ok(())
}

fn normalize_git_url(value: &str) -> OperationResult<String> {
    if let Some((scheme, _)) = value.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if matches!(scheme.as_str(), "https" | "http" | "file") {
            return Ok(value.to_string());
        }
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery git url scheme is not allowed",
        ));
    }

    let path = Path::new(value);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| {
                OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to resolve current directory: {err}"),
                )
            })?
            .join(path)
    };
    let canonical = path.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("battery local git source must exist: {err}"),
        )
    })?;
    Ok(canonical.display().to_string())
}

fn validate_git_ref(value: &str) -> OperationResult<()> {
    if value.trim().is_empty()
        || value.starts_with('-')
        || value.starts_with('+')
        || value.chars().any(|ch| {
            ch.is_control() || ch.is_whitespace() || matches!(ch, ':' | '*' | '?' | '[' | '\\')
        })
        || value.contains("..")
        || value.contains("@{")
    {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "battery ref is invalid",
        ));
    }
    Ok(())
}

fn verify_cache_origin_with_context(
    cache_path: &Path,
    expected_url: &str,
    ctx: &GitExecContext<'_>,
) -> OperationResult<()> {
    reject_unsafe_local_git_config(cache_path)?;
    let actual = run_git_capture_with_context(
        GitCommandSpec {
            program: "git".into(),
            args: vec![
                "-C".into(),
                cache_path.display().to_string(),
                "remote".into(),
                "get-url".into(),
                "origin".into(),
            ],
        },
        ctx,
    )?
    .trim()
    .to_string();
    if actual != expected_url {
        return Err(OperationError::new(
            OperationErrorCode::Conflict,
            "battery cache origin does not match registry git url; remove cache or re-sync from a clean registration",
        ));
    }
    Ok(())
}

fn url_contains_credentials(value: &str) -> bool {
    let Some((_, rest)) = value.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority.contains('@')
}

fn redacted_git_url(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let mut parts = rest.splitn(2, ['/', '?', '#']);
    let authority = parts.next().unwrap_or_default();
    if !authority.contains('@') {
        return value.to_string();
    }
    let suffix = &rest[authority.len()..];
    let host = authority.rsplit('@').next().unwrap_or(authority);
    format!("{scheme}://<redacted>@{host}{suffix}")
}

fn sanitize_file_component(value: &str) -> String {
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

fn run_git_with_context(spec: GitCommandSpec, ctx: &GitExecContext<'_>) -> OperationResult<()> {
    let output = git_command_with_context(&spec, ctx)
        .output()
        .map_err(|err| {
            OperationError::new(
                OperationErrorCode::GitFailed,
                format!("failed to spawn git: {err}"),
            )
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            sanitize_git_output(
                &String::from_utf8_lossy(&output.stderr),
                ctx.askpass.map(|a| a.token.as_str()),
            ),
        ))
    }
}

fn run_git_capture(spec: GitCommandSpec) -> OperationResult<String> {
    run_git_capture_with_policy(spec, GitTransportPolicy::Default)
}

fn run_git_capture_with_policy(
    spec: GitCommandSpec,
    policy: GitTransportPolicy,
) -> OperationResult<String> {
    run_git_capture_with_context(
        spec,
        &GitExecContext {
            policy,
            askpass: None,
            http_pin: None,
        },
    )
}

fn run_git_capture_with_context(
    spec: GitCommandSpec,
    ctx: &GitExecContext<'_>,
) -> OperationResult<String> {
    let output = git_command_with_context(&spec, ctx)
        .output()
        .map_err(|err| {
            OperationError::new(
                OperationErrorCode::GitFailed,
                format!("failed to spawn git: {err}"),
            )
        })?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(redact_token_in_text(
            &stdout,
            ctx.askpass.map(|a| a.token.as_str()),
        ))
    } else {
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            sanitize_git_output(
                &String::from_utf8_lossy(&output.stderr),
                ctx.askpass.map(|a| a.token.as_str()),
            ),
        ))
    }
}

/// Whether the installed `git` supports `http.curloptResolve`. This depends
/// only on the installed git binary, not on any per-sync state, so the
/// process-wide conclusive result is cached instead of spawning
/// `git help --config` on every battery sync. Transient execution failures are
/// not cached so a later sync can recover without restarting the process. The
/// mutex is held across the probe so concurrent callers single-flight onto
/// one subprocess spawn instead of a thundering herd. The lock is recovered
/// on poisoning rather than propagating the panic, since a poisoned lock here
/// would otherwise wedge every future battery sync in the process for good.
static GIT_HTTP_PINNING_SUPPORTED: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

/// Ceiling on the probe subprocess so a hung `git` can't stall the
/// single-flight lock (and therefore every queued battery sync) forever.
const GIT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Like `run_git_capture_with_context`, but kills the child and returns an
/// error if it doesn't exit within `timeout` instead of blocking forever.
/// Only safe for commands with small, bounded output (like `git help
/// --config`): stdout/stderr are read after exit, not streamed, so a command
/// that fills the OS pipe buffer before exiting could still deadlock.
fn run_git_capture_with_timeout(
    spec: GitCommandSpec,
    ctx: &GitExecContext<'_>,
    timeout: std::time::Duration,
) -> OperationResult<String> {
    let mut child = git_command_with_context(&spec, ctx)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            OperationError::new(
                OperationErrorCode::GitFailed,
                format!("failed to spawn git: {err}"),
            )
        })?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                let mut stderr = Vec::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr);
                }
                return if status.success() {
                    Ok(redact_token_in_text(
                        &stdout,
                        ctx.askpass.map(|a| a.token.as_str()),
                    ))
                } else {
                    Err(OperationError::new(
                        OperationErrorCode::GitFailed,
                        sanitize_git_output(
                            &String::from_utf8_lossy(&stderr),
                            ctx.askpass.map(|a| a.token.as_str()),
                        ),
                    ))
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(OperationError::new(
                        OperationErrorCode::GitFailed,
                        format!("git probe timed out after {timeout:?}"),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(err) => {
                return Err(OperationError::new(
                    OperationErrorCode::GitFailed,
                    format!("failed to wait for git: {err}"),
                ))
            }
        }
    }
}

fn assert_git_http_pinning_supported(ctx: &GitExecContext<'_>) -> OperationResult<()> {
    assert_git_http_pinning_supported_with(&GIT_HTTP_PINNING_SUPPORTED, || {
        run_git_capture_with_timeout(
            GitCommandSpec {
                program: "git".into(),
                args: vec!["--no-pager".into(), "help".into(), "--config".into()],
            },
            ctx,
            GIT_PROBE_TIMEOUT,
        )
    })
}

fn assert_git_http_pinning_supported_with<F>(
    cache: &std::sync::Mutex<Option<bool>>,
    probe: F,
) -> OperationResult<()>
where
    F: FnOnce() -> OperationResult<String>,
{
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let supported = match *guard {
        Some(supported) => supported,
        None => {
            let output = probe()?;
            let detected = output.lines().any(|key| key == "http.curloptResolve");
            *guard = Some(detected);
            detected
        }
    };
    drop(guard);
    if supported {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            "installed git does not support http.curloptResolve; refusing unpinned battery fetch",
        ))
    }
}

#[cfg(test)]
fn git_command(spec: &GitCommandSpec, policy: GitTransportPolicy) -> Command {
    git_command_with_context(
        spec,
        &GitExecContext {
            policy,
            askpass: None,
            http_pin: None,
        },
    )
}

fn git_command_with_context(spec: &GitCommandSpec, ctx: &GitExecContext<'_>) -> Command {
    let mut command = Command::new(&spec.program);
    command
        .args(["-c", "http.followRedirects=false"])
        .args(["-c", "http.proxy="]);
    if let Some(pin) = ctx.http_pin {
        command.args([
            "-c",
            &format!("http.curloptResolve={}", pin.curlopt_resolve()),
        ]);
    }
    command
        .args(&spec.args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", git_null_config_path())
        .env("GIT_CONFIG_SYSTEM", git_null_config_path())
        .env("GIT_ALLOW_PROTOCOL", ctx.policy.allowed_protocols())
        .env_remove("SSH_ASKPASS")
        .env_remove("GIT_SSH")
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_TEMPLATE_DIR")
        .env_remove("GIT_EXEC_PATH")
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_CONFIG_DIRS")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("OMAKURE_API_TOKEN")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("no_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("NO_PROXY");
    if let Some(askpass) = ctx.askpass {
        command
            .env("GIT_ASKPASS", &askpass.script_path)
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(pin) = ctx.http_pin {
            command.env("OMAKURE_GIT_AUTHORITY", pin.credential_authority());
        } else {
            command.env_remove("OMAKURE_GIT_AUTHORITY");
        }
    } else {
        command
            .env_remove("GIT_ASKPASS")
            .env_remove("OMAKURE_GIT_AUTHORITY");
    }
    command
}

#[cfg(windows)]
fn git_null_config_path() -> &'static str {
    "NUL"
}

#[cfg(not(windows))]
fn git_null_config_path() -> &'static str {
    "/dev/null"
}

fn verify_synced_checkout(cache_path: &Path, expected_commit: &str) -> OperationResult<()> {
    if !cache_path.join(".git").is_dir() {
        return Err(OperationError::new(
            OperationErrorCode::NotSynced,
            "battery cache is not a git checkout",
        ));
    }
    reject_unsafe_local_git_config(cache_path)?;
    let head = run_git_capture(GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-C".into(),
            cache_path.display().to_string(),
            "rev-parse".into(),
            "HEAD".into(),
        ],
    })?
    .trim()
    .to_string();
    if head != expected_commit {
        return Err(OperationError::new(
            OperationErrorCode::NotSynced,
            "battery cache HEAD does not match registry resolved commit",
        ));
    }
    let status = run_git_capture(GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-C".into(),
            cache_path.display().to_string(),
            "status".into(),
            "--porcelain".into(),
            "--untracked-files=all".into(),
        ],
    })?;
    if !status.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::Conflict,
            "battery cache has local modifications; run sync before install",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn sanitize_git_stderr(stderr: &str) -> String {
    sanitize_git_output(stderr, None)
}

fn sanitize_git_output(stderr: &str, token: Option<&str>) -> String {
    let mut message = redact_token_in_text(stderr.trim(), token);
    for part in stderr.split_whitespace() {
        if url_contains_credentials(part) {
            message = message.replace(part, &redacted_git_url(part));
        }
    }
    if message.is_empty() {
        "git command failed".to_string()
    } else {
        message
    }
}

fn redact_token_in_text(text: &str, token: Option<&str>) -> String {
    match token {
        Some(token) if !token.is_empty() && text.contains(token) => {
            text.replace(token, "<redacted>")
        }
        _ => text.to_string(),
    }
}

fn reject_unsafe_local_git_config(cache_path: &Path) -> OperationResult<()> {
    let config_path = cache_path.join(".git/config");
    reject_symlink_components(cache_path, Path::new(".git/config"), true)?;
    let config = fs::read_to_string(&config_path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to read local git config: {err}"),
        )
    })?;
    reject_unsafe_git_config_text(&config)?;
    let worktree_config = cache_path.join(".git/config.worktree");
    if worktree_config.exists() {
        reject_symlink_components(cache_path, Path::new(".git/config.worktree"), true)?;
        let config = fs::read_to_string(&worktree_config).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to read local worktree git config: {err}"),
            )
        })?;
        reject_unsafe_git_config_text(&config)?;
        return Err(OperationError::new(
            OperationErrorCode::Conflict,
            "battery cache uses local worktree git config",
        ));
    }
    Ok(())
}

fn reject_unsafe_git_config_text(config: &str) -> OperationResult<()> {
    let mut section = String::new();
    for raw_line in config.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            if section == "include"
                || section.starts_with("includeif ")
                || section.starts_with("includeif.")
            {
                return Err(OperationError::new(
                    OperationErrorCode::Conflict,
                    format!("battery cache has unsafe local git config: {section}"),
                ));
            }
            if section == "http" || section.starts_with("http ") {
                return Err(OperationError::new(
                    OperationErrorCode::Conflict,
                    format!("battery cache has unsafe local git config: {section}"),
                ));
            }
            continue;
        }
        let key = line
            .split_once('=')
            .map(|(key, _)| key)
            .unwrap_or(line)
            .trim()
            .to_ascii_lowercase();
        let unsafe_key = ((section == "credential" || section.starts_with("credential "))
            && key == "helper")
            || (section == "core"
                && (key == "askpass" || key == "sshcommand" || key == "worktree"))
            || (section == "extensions" && key == "worktreeconfig")
            || (section.starts_with("url ") && key == "insteadof")
            || (section.starts_with("remote ")
                && matches!(key.as_str(), "proxy" | "proxyauthmethod"));
        if unsafe_key {
            return Err(OperationError::new(
                OperationErrorCode::Conflict,
                format!("battery cache has unsafe local git config: {section}.{key}"),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifest {
    pub battery: BatteryManifestHeader,
    #[serde(default)]
    pub scripts: Vec<BatteryManifestScript>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifestHeader {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifestScript {
    pub id: String,
    pub path: PathBuf,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitTransportPolicy {
    Default,
    HttpsOnly,
}

impl GitTransportPolicy {
    fn allowed_protocols(self) -> &'static str {
        match self {
            Self::Default => "file:https:http",
            Self::HttpsOnly => "https",
        }
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
    write_atomic(path, contents.as_bytes(), "registry")
}

pub fn parse_manifest(contents: &str) -> OperationResult<BatteryManifest> {
    toml::from_str(contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("battery manifest is invalid: {err}"),
        )
    })
}

pub fn load_manifest(cache_path: &Path) -> OperationResult<BatteryManifest> {
    let manifest_rel = Path::new(MANIFEST_FILE);
    reject_symlink_components(cache_path, manifest_rel, true)?;
    let manifest_path = confined_existing_path(cache_path, manifest_rel)?;
    let contents = fs::read_to_string(&manifest_path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("failed to read battery manifest: {err}"),
        )
    })?;
    parse_manifest(&contents)
}

pub fn validate_manifest(cache_path: &Path, manifest: &BatteryManifest) -> OperationResult<()> {
    if manifest.battery.name.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            "battery manifest name is required",
        ));
    }
    let mut ids = HashSet::new();
    for script in &manifest.scripts {
        if !ids.insert(script.id.as_str()) {
            return Err(OperationError::new(
                OperationErrorCode::ManifestInvalid,
                format!("duplicate battery script id: {}", script.id),
            ));
        }
        validate_script_entry(cache_path, script)?;
    }
    Ok(())
}

fn validate_manifest_for_battery(
    cache_path: &Path,
    manifest: &BatteryManifest,
    battery_name: &str,
) -> OperationResult<()> {
    validate_manifest(cache_path, manifest)?;
    if manifest.battery.name != battery_name {
        return Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!(
                "battery manifest name '{}' does not match registration '{}'",
                manifest.battery.name, battery_name
            ),
        ));
    }
    Ok(())
}

pub fn validate_script_entry(
    cache_path: &Path,
    script: &BatteryManifestScript,
) -> OperationResult<PathBuf> {
    open_validated_script_entry(cache_path, script).map(|(path, _)| path)
}

fn open_validated_script_entry(
    cache_path: &Path,
    script: &BatteryManifestScript,
) -> OperationResult<(PathBuf, File)> {
    if script.id.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            "battery script id is required",
        ));
    }
    reject_unsafe_relative_path(&script.path)?;
    reject_reserved_install_path(&script.path)?;
    if script_kind(&script.path).is_none() {
        return Err(OperationError::new(
            OperationErrorCode::UnsupportedScript,
            format!(
                "unsupported battery script extension: {}",
                script.path.display()
            ),
        ));
    }

    // Check the raw joined path first: canonicalize follows symlinks, which
    // would hide the fact that the manifest pointed at a link.
    reject_symlink_components(cache_path, &script.path, true)?;
    let full_path = confined_existing_path(cache_path, &script.path)?;
    verify_tracked_blob(cache_path, &script.path)?;
    let mut file = open_existing_file_no_follow(&full_path)?;
    validate_script_schema_from_file(&script.path, &mut file)?;
    file.seek(SeekFrom::Start(0)).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to rewind battery script: {err}"),
        )
    })?;
    Ok((full_path, file))
}

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

fn reject_reserved_install_path(path: &Path) -> OperationResult<()> {
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

fn ensure_install_target_safe(
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

fn canonical_install_target_path(
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

fn ensure_installed_target_inside(
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

enum InstallState {
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
    fn rollback(&mut self) {
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

    fn cleanup(&mut self) {
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

#[cfg(unix)]
fn materialize_install(
    scripts_root: &Path,
    relative: &Path,
    installed_path: &Path,
    operation_path: &Path,
    source_file: &mut File,
    force: bool,
    target_existed: bool,
) -> OperationResult<InstallState> {
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
    copy_open_to_file(source_file, tmp_file)?;
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
    source_file: &mut File,
    force: bool,
    target_existed: bool,
) -> OperationResult<InstallState> {
    let (tmp_path, tmp_file) = unique_install_tmp_file(operation_path)?;
    copy_open_to_file(source_file, tmp_file)?;
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
    Ok(full)
}

pub fn reject_symlink(path: &Path) -> OperationResult<()> {
    let meta = fs::symlink_metadata(path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to inspect battery path: {err}"),
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("battery symlink is not allowed: {}", path.display()),
        ));
    }
    Ok(())
}

fn reject_symlink_components(
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

fn reject_existing_symlink_ancestors(path: &Path) -> OperationResult<()> {
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

fn verify_tracked_blob(cache_path: &Path, relative: &Path) -> OperationResult<()> {
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

fn copy_open_to_file(input: &mut File, mut output: File) -> OperationResult<()> {
    input.seek(SeekFrom::Start(0)).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to rewind battery script: {err}"),
        )
    })?;
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
fn open_existing_file_no_follow(path: &Path) -> OperationResult<File> {
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

#[cfg(unix)]
fn open_dir_no_follow(path: &Path) -> OperationResult<File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("path contains NUL byte: {}", path.display()),
        )
    })?;
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!(
                "failed to open install directory safely: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn create_new_file_at(
    parent: &File,
    target_name: &OsStr,
    suffix: &str,
) -> OperationResult<(OsString, File)> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let base = target_name.to_string_lossy();
    for attempt in 0..100u32 {
        let name = OsString::from(format!(
            ".{base}.{}.{}.{}",
            std::process::id(),
            attempt,
            suffix
        ));
        let c_name = cstring_os(&name)?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd >= 0 {
            return Ok((name, unsafe { File::from_raw_fd(fd) }));
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::AlreadyExists {
            continue;
        }
        return Err(OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create install {suffix} file: {err}"),
        ));
    }
    Err(OperationError::new(
        OperationErrorCode::Conflict,
        format!("failed to allocate a unique install {suffix} file"),
    ))
}

#[cfg(unix)]
fn open_existing_file_at_no_follow(parent: &File, name: &OsStr) -> OperationResult<File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let c_name = cstring_os(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        let err = io::Error::last_os_error();
        let code = if err.raw_os_error() == Some(libc::ELOOP) {
            OperationErrorCode::UnsafePath
        } else {
            OperationErrorCode::IoFailed
        };
        return Err(OperationError::new(
            code,
            format!("failed to open existing install target: {err}"),
        ));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    ensure_opened_regular_file(&file)?;
    Ok(file)
}

#[cfg(unix)]
fn renameat_file(parent: &File, from: &OsStr, to: &OsStr) -> OperationResult<()> {
    use std::os::fd::AsRawFd;

    let from = cstring_os(from)?;
    let to = cstring_os(to)?;
    let rc = unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::IoFailed,
            format!(
                "failed to install battery script: {}",
                io::Error::last_os_error()
            ),
        ))
    }
}

#[cfg(unix)]
fn linkat_file(parent: &File, from: &OsStr, to: &OsStr) -> OperationResult<()> {
    use std::os::fd::AsRawFd;

    let from = cstring_os(from)?;
    let to = cstring_os(to)?;
    let rc = unsafe {
        libc::linkat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            0,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        let err = io::Error::last_os_error();
        let code = if err.kind() == io::ErrorKind::AlreadyExists {
            OperationErrorCode::Conflict
        } else {
            OperationErrorCode::IoFailed
        };
        Err(OperationError::new(
            code,
            format!("failed to install battery script: {err}"),
        ))
    }
}

#[cfg(unix)]
fn unlinkat_file(parent: &File, name: &OsStr) -> OperationResult<()> {
    use std::os::fd::AsRawFd;

    let name = cstring_os(name)?;
    let rc = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::IoFailed,
            format!(
                "failed to remove install file: {}",
                io::Error::last_os_error()
            ),
        ))
    }
}

#[cfg(unix)]
fn cstring_os(value: &OsStr) -> OperationResult<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(value.as_bytes()).map_err(|_| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            "path component contains NUL byte",
        )
    })
}

#[cfg(not(unix))]
fn open_existing_file_no_follow(path: &Path) -> OperationResult<File> {
    let file = OpenOptions::new().read(true).open(path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to open battery script: {err}"),
        )
    })?;
    ensure_opened_regular_file(&file)?;
    Ok(file)
}

fn ensure_opened_regular_file(file: &File) -> OperationResult<()> {
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

fn write_atomic(path: &Path, contents: &[u8], label: &str) -> OperationResult<()> {
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

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn validate_script_schema_from_file(path: &Path, file: &mut File) -> OperationResult<()> {
    let prefixes = match script_kind(path) {
        Some(ScriptKind::Bash) => vec!["#"],
        Some(ScriptKind::PowerShell) => vec!["#", ";"],
        Some(ScriptKind::Python) => vec!["#"],
        None => {
            return Err(OperationError::new(
                OperationErrorCode::UnsupportedScript,
                format!("unsupported battery script extension: {}", path.display()),
            ));
        }
    };
    let mut contents = String::new();
    file.seek(SeekFrom::Start(0)).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to rewind battery script: {err}"),
        )
    })?;
    file.read_to_string(&mut contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to read battery script: {err}"),
        )
    })?;
    let block = extract_schema_block(&contents, &prefixes).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("battery script schema block is invalid: {err}"),
        )
    })?;
    parse_schema(&block).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("battery script schema is invalid: {err}"),
        )
    })?;
    Ok(())
}

pub fn git_clone_spec(git_url: &str, cache_path: &Path) -> GitCommandSpec {
    GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "protocol.ext.allow=never".into(),
            "-c".into(),
            "credential.helper=".into(),
            "clone".into(),
            "--no-recurse-submodules".into(),
            "--".into(),
            git_url.into(),
            cache_path.display().to_string(),
        ],
    }
}

pub fn git_fetch_spec(cache_path: &Path, requested_ref: &str) -> GitCommandSpec {
    GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-C".into(),
            cache_path.display().to_string(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "protocol.ext.allow=never".into(),
            "-c".into(),
            "credential.helper=".into(),
            "fetch".into(),
            "--no-recurse-submodules".into(),
            "origin".into(),
            "--".into(),
            requested_ref.into(),
        ],
    }
}

pub fn git_checkout_detached_spec(cache_path: &Path, commit: &str) -> GitCommandSpec {
    GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-C".into(),
            cache_path.display().to_string(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "protocol.ext.allow=never".into(),
            "checkout".into(),
            "--detach".into(),
            commit.into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tempfile::TempDir;

    fn valid_schema_script() -> String {
        r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {
#   "Name": "battery_script",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END
echo ok
"#
        .to_string()
    }

    #[test]
    fn add_battery_request_carries_cli_inputs_without_transport_details() {
        let request = AddBatteryRequest {
            name: "azure".into(),
            git_url: "https://example.invalid/azure.git".into(),
            requested_ref: "main".into(),
            token_ref: None,
        };

        assert_eq!(request.name, "azure");
        assert_eq!(request.requested_ref, "main");
    }

    #[test]
    fn git_http_pinning_probe_retries_after_transient_failure() {
        let cache = std::sync::Mutex::new(None);
        let attempts = Cell::new(0);
        let err = assert_git_http_pinning_supported_with(&cache, || {
            attempts.set(attempts.get() + 1);
            Err(OperationError::new(
                OperationErrorCode::GitFailed,
                "temporary spawn failure",
            ))
        })
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::GitFailed);
        assert!(cache.lock().unwrap().is_none());

        assert_git_http_pinning_supported_with(&cache, || {
            attempts.set(attempts.get() + 1);
            Ok("http.curloptResolve\n".into())
        })
        .unwrap();
        assert_eq!(attempts.get(), 2);

        assert_git_http_pinning_supported_with(&cache, || {
            panic!("conclusive probe result should be cached")
        })
        .unwrap();
    }

    #[test]
    fn git_http_pinning_probe_caches_definitive_unsupported_result() {
        let cache = std::sync::Mutex::new(None);
        let err = assert_git_http_pinning_supported_with(&cache, || Ok("other.key\n".into()))
            .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::GitFailed);
        assert_eq!(*cache.lock().unwrap(), Some(false));
        assert_git_http_pinning_supported_with(&cache, || {
            panic!("conclusive probe result should be cached")
        })
        .unwrap_err();
    }

    #[test]
    fn git_http_pinning_probe_single_flights_concurrent_callers() {
        let cache = std::sync::Arc::new(std::sync::Mutex::new(None));
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let start_barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let cache = std::sync::Arc::clone(&cache);
                let attempts = std::sync::Arc::clone(&attempts);
                let start_barrier = std::sync::Arc::clone(&start_barrier);
                std::thread::spawn(move || {
                    start_barrier.wait();
                    assert_git_http_pinning_supported_with(&cache, || {
                        attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        Ok("http.curloptResolve\n".into())
                    })
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "concurrent callers must single-flight onto one probe spawn"
        );
    }

    #[test]
    fn git_http_pinning_probe_single_flights_through_repeated_transient_failures() {
        let cache = std::sync::Arc::new(std::sync::Mutex::new(None));
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let start_barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let cache = std::sync::Arc::clone(&cache);
                let attempts = std::sync::Arc::clone(&attempts);
                let in_flight = std::sync::Arc::clone(&in_flight);
                let max_in_flight = std::sync::Arc::clone(&max_in_flight);
                let start_barrier = std::sync::Arc::clone(&start_barrier);
                std::thread::spawn(move || {
                    start_barrier.wait();
                    loop {
                        let result = assert_git_http_pinning_supported_with(&cache, || {
                            let concurrent =
                                in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            max_in_flight
                                .fetch_max(concurrent, std::sync::atomic::Ordering::SeqCst);
                            std::thread::sleep(std::time::Duration::from_millis(5));
                            let attempt =
                                attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            if attempt < 5 {
                                Err(OperationError::new(
                                    OperationErrorCode::GitFailed,
                                    "temporary spawn failure",
                                ))
                            } else {
                                Ok("http.curloptResolve\n".into())
                            }
                        });
                        if result.is_ok() {
                            return;
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            max_in_flight.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "at most one probe may run at a time, even while callers retry after transient failures"
        );
        assert!(
            attempts.load(std::sync::atomic::Ordering::SeqCst) >= 6,
            "expected 5 transient failures followed by 1 successful probe"
        );
    }

    #[test]
    fn install_request_keeps_force_explicit() {
        let request = InstallBatteryScriptRequest {
            battery_name: "azure".into(),
            script_id: "azure.rg-list-all".into(),
            force: false,
        };

        assert!(!request.force);
    }

    #[test]
    fn battery_paths_live_under_omakure_metadata() {
        let ws = Workspace::new(PathBuf::from("/tmp/omakure-battery-test"));
        let paths = BatteryPaths::for_workspace(&ws);

        assert_eq!(paths.registry_path, ws.omakure_dir().join("batteries.json"));
        assert_eq!(
            paths.cache_path_for("azure"),
            ws.omakure_dir().join("batteries/cache/azure")
        );
        assert_eq!(
            paths.installed_root,
            ws.omakure_dir().join("batteries/installed")
        );
    }

    #[test]
    fn git_command_removes_api_token() {
        let command = git_command(
            &GitCommandSpec {
                program: "git".into(),
                args: vec!["status".into()],
            },
            GitTransportPolicy::Default,
        );

        assert!(command
            .get_envs()
            .any(|(k, v)| k == "OMAKURE_API_TOKEN" && v.is_none()));
    }

    #[test]
    fn missing_registry_reads_as_empty_versioned_registry() {
        let dir = TempDir::new().unwrap();
        let registry = read_registry(&dir.path().join(".omakure/batteries.json")).unwrap();

        assert_eq!(registry.version, REGISTRY_VERSION);
        assert!(registry.batteries.is_empty());
    }

    #[test]
    fn registry_round_trips_atomically() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".omakure/batteries.json");
        let registry = BatteryRegistry {
            version: REGISTRY_VERSION,
            batteries: vec![BatterySummary {
                name: "azure".into(),
                git_url: "https://example.invalid/azure.git".into(),
                requested_ref: "main".into(),
                resolved_commit: None,
                cache_path: PathBuf::from(".omakure/batteries/cache/azure"),
                last_synced_at: None,
                auth: None,
            }],
        };

        write_registry(&path, &registry).unwrap();
        assert_eq!(read_registry(&path).unwrap(), registry);
    }

    #[test]
    fn invalid_registry_is_reported() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("batteries.json");
        fs::write(&path, "not json").unwrap();

        let err = read_registry(&path).unwrap_err();
        assert_eq!(err.code, OperationErrorCode::RegistryInvalid);
    }

    #[test]
    fn registry_rejects_tampered_cache_paths() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("batteries.json");
        fs::write(
            &path,
            r#"{
  "version": 1,
  "batteries": [{
    "name": "azure",
    "git_url": "https://example.invalid/azure.git",
    "requested_ref": "main",
    "resolved_commit": null,
    "cache_path": "../../outside",
    "last_synced_at": null
  }]
}"#,
        )
        .unwrap();

        let err = read_registry(&path).unwrap_err();
        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn manifest_parses_script_entries() {
        let manifest = parse_manifest(
            r#"
[battery]
name = "azure"
version = "0.1.0"
description = "Azure scripts"

[[scripts]]
id = "azure.list"
path = "scripts/list.sh"
description = "List"
tags = ["azure"]
"#,
        )
        .unwrap();

        assert_eq!(manifest.battery.name, "azure");
        assert_eq!(manifest.scripts[0].id, "azure.list");
        assert_eq!(manifest.scripts[0].path, PathBuf::from("scripts/list.sh"));
    }

    #[test]
    fn duplicate_manifest_script_ids_are_rejected() {
        let dir = TempDir::new().unwrap();
        let cache = dir.path();
        write_manifest_and_script(cache);
        fs::write(cache.join("scripts/other.sh"), valid_schema_script()).unwrap();
        let manifest = parse_manifest(
            r#"
[battery]
name = "azure"
version = "0.1.0"

[[scripts]]
id = "same"
path = "scripts/list.sh"

[[scripts]]
id = "same"
path = "scripts/other.sh"
"#,
        )
        .unwrap();

        let err = validate_manifest(cache, &manifest).unwrap_err();
        assert_eq!(err.code, OperationErrorCode::ManifestInvalid);
    }

    #[test]
    fn unsafe_manifest_paths_are_rejected() {
        let script = BatteryManifestScript {
            id: "bad".into(),
            path: PathBuf::from("../escape.sh"),
            description: None,
            tags: Vec::new(),
        };

        let err = validate_script_entry(Path::new("/tmp"), &script).unwrap_err();
        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn unsupported_script_extensions_are_rejected() {
        let script = BatteryManifestScript {
            id: "bad".into(),
            path: PathBuf::from("scripts/readme.md"),
            description: None,
            tags: Vec::new(),
        };

        let err = validate_script_entry(Path::new("/tmp"), &script).unwrap_err();
        assert_eq!(err.code, OperationErrorCode::UnsupportedScript);
    }

    #[test]
    fn script_entry_requires_valid_schema() {
        let dir = TempDir::new().unwrap();
        let script_dir = dir.path().join("scripts");
        fs::create_dir_all(&script_dir).unwrap();
        fs::write(script_dir.join("list.sh"), valid_schema_script()).unwrap();
        init_cache_git(dir.path());
        let script = BatteryManifestScript {
            id: "azure.list".into(),
            path: PathBuf::from("scripts/list.sh"),
            description: None,
            tags: Vec::new(),
        };

        let path = validate_script_entry(dir.path(), &script).unwrap();
        assert!(path.ends_with("scripts/list.sh"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_scripts_are_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let script_dir = dir.path().join("scripts");
        fs::create_dir_all(&script_dir).unwrap();
        let target = script_dir.join("target.sh");
        fs::write(&target, valid_schema_script()).unwrap();
        symlink(&target, script_dir.join("link.sh")).unwrap();
        let script = BatteryManifestScript {
            id: "azure.link".into(),
            path: PathBuf::from("scripts/link.sh"),
            description: None,
            tags: Vec::new(),
        };

        let err = validate_script_entry(dir.path(), &script).unwrap_err();
        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[cfg(unix)]
    #[test]
    fn manifest_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside.toml");
        fs::write(
            &outside,
            "[battery]\nname = \"azure\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        symlink(&outside, dir.path().join(MANIFEST_FILE)).unwrap();

        let err = load_manifest(dir.path()).unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn manifest_name_must_match_registered_battery_name() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        let cache = paths.cache_path_for("azure");
        write_manifest_and_script(&cache);
        fs::write(
            cache.join(MANIFEST_FILE),
            r#"
[battery]
name = "other"
version = "0.1.0"

[[scripts]]
id = "azure.list"
path = "scripts/list.sh"
"#,
        )
        .unwrap();
        let commit = init_cache_git(&cache);
        write_registry(&paths.registry_path, &synced_registry_with_commit(commit)).unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::ManifestInvalid);
    }

    #[test]
    fn ignored_untracked_manifest_script_is_rejected() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        let cache = paths.cache_path_for("azure");
        fs::create_dir_all(cache.join("scripts")).unwrap();
        fs::write(cache.join(".gitignore"), "scripts/ignored.sh\n").unwrap();
        fs::write(cache.join("scripts/ignored.sh"), valid_schema_script()).unwrap();
        fs::write(
            cache.join(MANIFEST_FILE),
            r#"
[battery]
name = "azure"
version = "0.1.0"

[[scripts]]
id = "azure.ignored"
path = "scripts/ignored.sh"
"#,
        )
        .unwrap();
        run_test_git(&["init", "-b", "main"], &cache);
        run_test_git(&["add", MANIFEST_FILE, ".gitignore"], &cache);
        run_test_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "initial",
            ],
            &cache,
        );
        write_registry(
            &paths.registry_path,
            &synced_registry_with_commit(cache_head(&cache)),
        )
        .unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::ManifestInvalid);
    }

    #[test]
    fn git_command_disables_prompts_and_external_config() {
        let spec = GitCommandSpec {
            program: "git".into(),
            args: vec!["status".into()],
        };
        let command = git_command(&spec, GitTransportPolicy::Default);
        let envs: Vec<_> = command.get_envs().collect();

        assert!(envs.iter().any(|(key, value)| {
            *key == "GIT_TERMINAL_PROMPT" && value.map(|v| v == "0").unwrap_or(false)
        }));
        assert!(envs.iter().any(|(key, value)| {
            *key == "GIT_ALLOW_PROTOCOL" && value.map(|v| v == "file:https:http").unwrap_or(false)
        }));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "GIT_ASKPASS" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "SSH_ASKPASS" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "GIT_SSH" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "GIT_SSH_COMMAND" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "GIT_TEMPLATE_DIR" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "GIT_EXEC_PATH" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "HOME" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "XDG_CONFIG_HOME" && value.is_none()));
        assert!(envs.iter().any(|(key, value)| {
            *key == "GIT_CONFIG_NOSYSTEM" && value.map(|v| v == "1").unwrap_or(false)
        }));
        assert!(envs.iter().any(|(key, value)| {
            *key == "GIT_ALLOW_PROTOCOL" && value.map(|v| v == "file:https:http").unwrap_or(false)
        }));
        assert!(envs.iter().any(|(key, value)| {
            *key == "GIT_CONFIG_GLOBAL"
                && value.map(|v| v == git_null_config_path()).unwrap_or(false)
        }));
        assert!(envs.iter().any(|(key, value)| {
            *key == "GIT_CONFIG_SYSTEM"
                && value.map(|v| v == git_null_config_path()).unwrap_or(false)
        }));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "GIT_CONFIG_COUNT" && value.is_none()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == "GIT_CONFIG_PARAMETERS" && value.is_none()));
    }

    #[test]
    fn git_command_can_restrict_protocols_to_https() {
        let spec = GitCommandSpec {
            program: "git".into(),
            args: vec!["status".into()],
        };
        let command = git_command(&spec, GitTransportPolicy::HttpsOnly);
        let envs: Vec<_> = command.get_envs().collect();

        assert!(envs.iter().any(|(key, value)| {
            *key == "GIT_ALLOW_PROTOCOL" && value.map(|v| v == "https").unwrap_or(false)
        }));
    }

    #[test]
    fn git_http_command_disables_redirects_proxies_and_pins_verified_host() {
        let spec = GitCommandSpec {
            program: "git".into(),
            args: vec!["fetch".into()],
        };
        let pin = GitHttpPin {
            host: "git.example.test".into(),
            port: 8443,
            address: "203.0.113.10".parse().unwrap(),
            credential_authority: "git.example.test:8443".into(),
        };
        let command = git_command_with_context(
            &spec,
            &GitExecContext {
                policy: GitTransportPolicy::HttpsOnly,
                askpass: None,
                http_pin: Some(&pin),
            },
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "http.followRedirects=false"]));
        assert!(args.windows(2).any(|pair| pair == ["-c", "http.proxy="]));
        assert!(args.windows(2).any(|pair| {
            pair == [
                "-c",
                "http.curloptResolve=git.example.test:8443:203.0.113.10",
            ]
        }));
        for proxy in [
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
        ] {
            assert!(command
                .get_envs()
                .any(|(key, value)| key == proxy && value.is_none()));
        }
    }

    #[test]
    fn git_http_pin_formats_ipv6_for_curl_and_uses_url_port() {
        let pin = resolve_public_git_endpoint("https://[2606:4700:4700::1111]:9443/repository.git")
            .unwrap()
            .unwrap();

        assert_eq!(pin.host, "2606:4700:4700::1111");
        assert_eq!(pin.port, 9443);
        assert_eq!(pin.credential_authority(), "[2606:4700:4700::1111]:9443");
        assert_eq!(
            pin.curlopt_resolve(),
            "[2606:4700:4700::1111]:9443:[2606:4700:4700::1111]"
        );
    }

    #[cfg(unix)]
    #[test]
    fn intermediate_symlink_directories_are_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("list.sh"), valid_schema_script()).unwrap();
        symlink(&real, dir.path().join("scripts")).unwrap();
        let script = BatteryManifestScript {
            id: "azure.list".into(),
            path: PathBuf::from("scripts/list.sh"),
            description: None,
            tags: Vec::new(),
        };

        let err = validate_script_entry(dir.path(), &script).unwrap_err();
        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn reserved_install_paths_are_rejected() {
        for path in [".omakure/foo.sh", ".history/foo.sh", ".git/hooks/foo.sh"] {
            let script = BatteryManifestScript {
                id: "bad".into(),
                path: PathBuf::from(path),
                description: None,
                tags: Vec::new(),
            };

            let err = validate_script_entry(Path::new("/tmp"), &script).unwrap_err();
            assert_eq!(err.code, OperationErrorCode::UnsafePath, "{path}");
        }
    }

    #[test]
    fn git_specs_disable_hooks_submodules_and_checkout_detached() {
        let cache = Path::new("/tmp/cache/azure");
        let clone = git_clone_spec("https://example.invalid/azure.git", cache);
        assert_eq!(clone.program, "git");
        assert!(clone.args.contains(&"core.hooksPath=/dev/null".to_string()));
        assert!(clone.args.contains(&"protocol.ext.allow=never".to_string()));
        assert!(clone.args.contains(&"--no-recurse-submodules".to_string()));

        let fetch = git_fetch_spec(cache, "main");
        assert!(fetch.args.contains(&"--no-recurse-submodules".to_string()));

        let checkout = git_checkout_detached_spec(cache, "0123456789abcdef");
        assert!(checkout.args.contains(&"--detach".to_string()));
    }

    #[test]
    fn local_git_config_cannot_override_http_containment() {
        for config in [
            "[http]\n\tfollowRedirects = true\n",
            "[http \"https://example.test\"]\n\tcurloptResolve = example.test:443:127.0.0.1\n",
            "[remote \"origin\"]\n\tproxy = http://127.0.0.1:8080\n",
        ] {
            assert_eq!(
                reject_unsafe_git_config_text(config).unwrap_err().code,
                OperationErrorCode::Conflict,
                "{config}"
            );
        }
    }

    #[test]
    fn git_inputs_reject_options_controls_and_credentials() {
        assert_eq!(
            validate_git_url("-https://example.invalid/repo.git")
                .unwrap_err()
                .code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            validate_git_url("https://user:secret@example.invalid/repo.git")
                .unwrap_err()
                .code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            validate_git_ref("--upload-pack=sh").unwrap_err().code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            validate_git_ref("main branch").unwrap_err().code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            validate_git_ref("+main:refs/heads/main").unwrap_err().code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            validate_git_ref("main:refs/heads/main").unwrap_err().code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            validate_git_ref("feature@{1}").unwrap_err().code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            validate_git_url("https://example.invalid/repo.git?token=secret")
                .unwrap_err()
                .code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            normalize_git_url("ssh://example.invalid/repo.git")
                .unwrap_err()
                .code,
            OperationErrorCode::InvalidInput
        );
        assert_eq!(
            normalize_git_url("git@example.invalid:repo.git")
                .unwrap_err()
                .code,
            OperationErrorCode::InvalidInput
        );
    }

    #[test]
    fn local_git_source_is_stored_as_canonical_absolute_path() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();

        let normalized = normalize_git_url(repo.to_str().unwrap()).unwrap();

        assert_eq!(
            normalized,
            repo.canonicalize().unwrap().display().to_string()
        );
        assert!(Path::new(&normalized).is_absolute());
    }

    #[test]
    fn git_stderr_redacts_credentials() {
        let msg = sanitize_git_stderr(
            "fatal: unable to access 'https://user:secret@example.invalid/repo.git'",
        );
        assert!(!msg.contains("secret"));
        assert!(msg.contains("<redacted>"));
    }

    fn write_manifest_and_script(cache: &Path) {
        fs::create_dir_all(cache.join("scripts")).unwrap();
        fs::write(cache.join("scripts/list.sh"), valid_schema_script()).unwrap();
        fs::write(
            cache.join(MANIFEST_FILE),
            r#"
[battery]
name = "azure"
version = "0.1.0"
description = "Azure scripts"

[[scripts]]
id = "azure.list"
path = "scripts/list.sh"
description = "List"
tags = ["azure"]
"#,
        )
        .unwrap();
    }

    fn workspace_in(dir: &TempDir) -> Workspace {
        let ws = Workspace::new(dir.path().to_path_buf());
        ws.ensure_layout().unwrap();
        ws
    }

    fn synced_registry_with_commit(commit: impl Into<String>) -> BatteryRegistry {
        BatteryRegistry {
            version: REGISTRY_VERSION,
            batteries: vec![BatterySummary {
                name: "azure".into(),
                git_url: "https://example.invalid/azure.git".into(),
                requested_ref: "main".into(),
                resolved_commit: Some(commit.into()),
                cache_path: PathBuf::from(".omakure/batteries/cache/azure"),
                last_synced_at: Some("2026-07-07T00:00:00Z".into()),
                auth: None,
            }],
        }
    }

    fn cache_head(cache: &Path) -> String {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(cache)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_cache_git(cache: &Path) -> String {
        run_test_git(&["init", "-b", "main"], cache);
        run_test_git(&["add", "."], cache);
        run_test_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "initial",
            ],
            cache,
        );
        cache_head(cache)
    }

    fn write_synced_cache_and_registry(ws: &Workspace) -> PathBuf {
        let paths = BatteryPaths::for_workspace(ws);
        let cache = paths.cache_path_for("azure");
        write_manifest_and_script(&cache);
        let commit = init_cache_git(&cache);
        write_registry(&paths.registry_path, &synced_registry_with_commit(commit)).unwrap();
        cache
    }

    #[test]
    fn list_batteries_returns_registry_summaries() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        write_registry(
            &paths.registry_path,
            &synced_registry_with_commit("0123456789abcdef0123456789abcdef01234567"),
        )
        .unwrap();

        let batteries = list_batteries(&ws).unwrap();

        assert_eq!(batteries.len(), 1);
        assert_eq!(batteries[0].name, "azure");
    }

    #[test]
    fn inspect_battery_loads_and_validates_manifest() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);

        let response = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap();

        assert_eq!(response.summary.name, "azure");
        assert_eq!(response.cache_status, BatteryCacheStatus::Synced);
        assert_eq!(response.manifest.scripts[0].id, "azure.list");
    }

    #[test]
    fn inspect_missing_battery_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "missing".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::NotFound);
    }

    #[test]
    fn inspect_unsynced_battery_returns_not_synced() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        let mut registry = synced_registry_with_commit("0123456789abcdef0123456789abcdef01234567");
        registry.batteries[0].resolved_commit = None;
        write_registry(&paths.registry_path, &registry).unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::NotSynced);
    }

    #[test]
    fn inspect_rejects_cache_with_mismatched_head() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        let cache = paths.cache_path_for("azure");
        write_manifest_and_script(&cache);
        init_cache_git(&cache);
        write_registry(
            &paths.registry_path,
            &synced_registry_with_commit("0123456789abcdef0123456789abcdef01234567"),
        )
        .unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::NotSynced);
    }

    #[test]
    fn inspect_rejects_dirty_or_untracked_cache() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let cache = write_synced_cache_and_registry(&ws);
        fs::write(cache.join("untracked.txt"), "dirty").unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::Conflict);
    }

    #[test]
    fn inspect_rejects_unsafe_local_git_config() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let cache = write_synced_cache_and_registry(&ws);
        run_test_git(&["config", "credential.helper", "!/bin/false"], &cache);

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::Conflict);
    }

    #[test]
    fn inspect_rejects_local_git_include_without_evaluating_it() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let cache = write_synced_cache_and_registry(&ws);
        fs::write(
            cache.join(".git/config"),
            r#"[core]
	repositoryformatversion = 0
	filemode = true
	bare = false
[includeIf "gitdir:/tmp/"]
	path = /tmp/malicious.gitconfig
"#,
        )
        .unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::Conflict);
    }

    #[test]
    fn inspect_rejects_worktree_git_config() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let cache = write_synced_cache_and_registry(&ws);
        fs::write(
            cache.join(".git/config.worktree"),
            r#"[credential]
	helper = !/bin/false
"#,
        )
        .unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::Conflict);
    }

    #[test]
    fn inspect_rejects_worktree_config_extension() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let cache = write_synced_cache_and_registry(&ws);
        fs::write(
            cache.join(".git/config"),
            r#"[core]
	repositoryformatversion = 0
	filemode = true
	bare = false
[extensions]
	worktreeConfig = true
"#,
        )
        .unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::Conflict);
    }

    #[test]
    fn inspect_rejects_core_worktree_config() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let cache = write_synced_cache_and_registry(&ws);
        fs::write(
            cache.join(".git/config"),
            r#"[core]
	repositoryformatversion = 0
	filemode = true
	bare = false
	worktree = /tmp/elsewhere
"#,
        )
        .unwrap();

        let err = inspect_battery(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::Conflict);
    }

    #[test]
    fn list_battery_scripts_maps_valid_manifest_scripts() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);

        let scripts = list_battery_scripts(
            &ws,
            InspectBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap();

        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].id, "azure.list");
        assert_eq!(scripts[0].tags, vec!["azure"]);
    }

    #[test]
    fn add_battery_stores_token_ref_auth_without_plaintext() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let plaintext = "super-secret-battery-token-value";
        std::env::set_var("OMAKURE_BATTERY_TOKEN_TEST", plaintext);

        let summary = add_battery(
            &ws,
            AddBatteryRequest {
                name: "private".into(),
                git_url: "https://example.invalid/private.git".into(),
                requested_ref: "main".into(),
                token_ref: Some("secret://env/OMAKURE_BATTERY_TOKEN_TEST".into()),
            },
        )
        .unwrap();

        assert_eq!(
            summary.auth.as_ref().map(|a| a.method.clone()),
            Some(BatteryAuthMethod::HttpsTokenRef)
        );
        assert_eq!(
            summary.auth.as_ref().map(|a| a.token_ref.as_str()),
            Some("secret://env/OMAKURE_BATTERY_TOKEN_TEST")
        );
        let registry_text =
            fs::read_to_string(BatteryPaths::for_workspace(&ws).registry_path).unwrap();
        assert!(registry_text.contains("secret://env/OMAKURE_BATTERY_TOKEN_TEST"));
        assert!(registry_text.contains("https_token_ref"));
        assert!(!registry_text.contains(plaintext));
        std::env::remove_var("OMAKURE_BATTERY_TOKEN_TEST");
    }

    #[test]
    fn add_battery_rejects_token_ref_on_non_https() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let repo = create_battery_repo();
        let err = add_battery(
            &ws,
            AddBatteryRequest {
                name: "local".into(),
                git_url: repo.path().display().to_string(),
                requested_ref: "main".into(),
                token_ref: Some("secret://env/TOKEN".into()),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, OperationErrorCode::InvalidInput);
    }

    #[test]
    fn prepare_git_askpass_writes_0600_files_and_redacts_token() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let plaintext = "askpass-redact-me-token-xyz";
        std::env::set_var("OMAKURE_ASKPASS_TOKEN", plaintext);
        let auth = BatteryAuth {
            method: BatteryAuthMethod::HttpsTokenRef,
            token_ref: "secret://env/OMAKURE_ASKPASS_TOKEN".into(),
        };
        let guard = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
            .unwrap()
            .expect("askpass");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&guard.script_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
            let token_mode = fs::metadata(guard.dir.join("token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(token_mode, 0o600);
        }
        let redacted = sanitize_git_output(
            &format!("fatal: Authentication failed for token {plaintext}"),
            Some(plaintext),
        );
        assert!(!redacted.contains(plaintext));
        assert!(redacted.contains("<redacted>"));
        let script = fs::read_to_string(&guard.script_path).unwrap();
        assert!(script.contains("\"$DIR/token\""));
        assert!(!script.contains(plaintext));
        assert!(!script.contains(guard.dir.to_string_lossy().as_ref()));
        drop(guard);
        std::env::remove_var("OMAKURE_ASKPASS_TOKEN");
    }

    #[test]
    fn prepare_git_askpass_uses_distinct_directories_per_call() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        std::env::set_var("OMAKURE_ASKPASS_DISTINCT", "tok-a");
        let auth = BatteryAuth {
            method: BatteryAuthMethod::HttpsTokenRef,
            token_ref: "secret://env/OMAKURE_ASKPASS_DISTINCT".into(),
        };
        let a = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
            .unwrap()
            .unwrap();
        let b = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
            .unwrap()
            .unwrap();
        assert_ne!(a.dir, b.dir);
        assert!(a.dir.exists());
        assert!(b.dir.exists());
        drop(a);
        drop(b);
        std::env::remove_var("OMAKURE_ASKPASS_DISTINCT");
    }

    #[test]
    fn assert_local_battery_allowed_rejects_file_urls_when_disabled() {
        let err = assert_local_battery_allowed(false, "file:///tmp/repo.git").unwrap_err();
        assert_eq!(err.code, OperationErrorCode::Forbidden);
        assert!(assert_local_battery_allowed(true, "file:///tmp/repo.git").is_ok());
        assert!(assert_local_battery_allowed(false, "https://example.invalid/repo.git").is_ok());
    }

    #[test]
    fn assert_public_git_host_rejects_private_and_metadata_literals() {
        for url in [
            "https://127.0.0.1/repo.git",
            "https://10.0.0.5/repo.git",
            "https://192.168.1.10/repo.git",
            "https://172.16.4.4/repo.git",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/repo.git",
            "https://192.0.0.1/repo.git",
            "https://192.88.99.1/repo.git",
            "https://198.18.0.1/repo.git",
            "https://224.0.0.1/repo.git",
            "https://240.0.0.1/repo.git",
            "https://168.63.129.16/repo.git",
            "https://[::1]/repo.git",
            "https://[fd00::1]/repo.git",
            "https://[fe80::1]/repo.git",
            "https://[2001:db8::1]/repo.git",
            "https://[2001:2::1]/repo.git",
            "https://0.0.0.0/repo.git",
        ] {
            let err = assert_public_git_host(url).unwrap_err();
            assert_eq!(err.code, OperationErrorCode::Forbidden, "{url}");
        }
    }

    #[test]
    fn assert_public_git_host_rejects_ipv4_compatible_and_nat64_literals() {
        for url in [
            // ::127.0.0.1 (IPv4-compatible loopback)
            "https://[::7f00:1]/repo.git",
            // ::10.0.0.5
            "https://[::a00:5]/repo.git",
            // NAT64 64:ff9b::169.254.169.254 (metadata)
            "https://[64:ff9b::a9fe:a9fe]/latest",
            // NAT64 64:ff9b::10.0.0.5
            "https://[64:ff9b::a00:5]/repo.git",
        ] {
            let err = assert_public_git_host(url).unwrap_err();
            assert_eq!(err.code, OperationErrorCode::Forbidden, "{url}");
        }
        // A public IPv4 embedded in NAT64 stays allowed.
        assert!(assert_public_git_host("https://[64:ff9b::808:808]/repo.git").is_ok());
    }

    #[test]
    fn assert_public_git_host_rejects_6to4_teredo_and_nat64_local_literals() {
        for url in [
            // 6to4 2002:<v4>::  → 10.0.0.5
            "https://[2002:a00:5::]/repo.git",
            // 6to4 → 127.0.0.1
            "https://[2002:7f00:1::]/repo.git",
            // Teredo 2001:0000:...:~client → ~f5ff:fffa == 10.0.0.5
            "https://[2001:0:0:0:0:0:f5ff:fffa]/repo.git",
            // NAT64 local-use prefix 64:ff9b:1::/48 (blocked wholesale)
            "https://[64:ff9b:1::a00:5]/repo.git",
            "https://[64:ff9b:1::808:808]/repo.git",
        ] {
            let err = assert_public_git_host(url).unwrap_err();
            assert_eq!(err.code, OperationErrorCode::Forbidden, "{url}");
        }
        // A 6to4 address embedding a PUBLIC gateway stays allowed.
        assert!(assert_public_git_host("https://[2002:808:808::]/repo.git").is_ok());
    }

    #[test]
    fn assert_public_git_host_rejects_credentialed_metadata_host() {
        // Userinfo must not smuggle a private host past the check.
        let err = assert_public_git_host("https://user@169.254.169.254/latest").unwrap_err();
        assert_eq!(err.code, OperationErrorCode::Forbidden);
    }

    #[test]
    fn literal_host_guard_blocks_private_literals_but_is_hermetic() {
        // Blocks literal private/metadata IPs...
        for url in [
            "https://127.0.0.1/repo.git",
            "https://169.254.169.254/latest",
            "https://[::1]/repo.git",
        ] {
            assert_eq!(
                assert_git_url_host_public_literal(url).unwrap_err().code,
                OperationErrorCode::Forbidden,
                "{url}"
            );
        }
        // ...but never resolves DNS, so non-literal hosts pass regardless of
        // whether they resolve (keeps registration hermetic).
        assert!(assert_git_url_host_public_literal("https://example.invalid/x.git").is_ok());
        assert!(assert_git_url_host_public_literal("https://8.8.8.8/x.git").is_ok());
        assert!(assert_git_url_host_public_literal("file:///tmp/x.git").is_ok());
    }

    #[test]
    fn assert_public_git_host_allows_public_literal_and_skips_non_network() {
        assert!(assert_public_git_host("https://8.8.8.8/repo.git").is_ok());
        assert!(assert_public_git_host("https://[2606:4700:4700::1111]/repo.git").is_ok());
        // file / local sources are vetted elsewhere.
        assert!(assert_public_git_host("file:///tmp/repo.git").is_ok());
        assert!(assert_public_git_host("/tmp/local/repo.git").is_ok());
    }

    #[test]
    fn git_command_with_askpass_sets_git_askpass_env() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        std::env::set_var("OMAKURE_ASKPASS_ENV", "tok");
        let auth = BatteryAuth {
            method: BatteryAuthMethod::HttpsTokenRef,
            token_ref: "secret://env/OMAKURE_ASKPASS_ENV".into(),
        };
        let guard = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
            .unwrap()
            .unwrap();
        let pin = GitHttpPin {
            host: "git.example.test".into(),
            port: 443,
            address: "203.0.113.10".parse().unwrap(),
            credential_authority: "git.example.test".into(),
        };
        let command = git_command_with_context(
            &GitCommandSpec {
                program: "git".into(),
                args: vec!["status".into()],
            },
            &GitExecContext {
                policy: GitTransportPolicy::HttpsOnly,
                askpass: Some(&guard),
                http_pin: Some(&pin),
            },
        );
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| {
            *k == "GIT_ASKPASS"
                && v.map(|p| p == guard.script_path.as_os_str())
                    .unwrap_or(false)
        }));
        assert!(envs
            .iter()
            .any(|(k, v)| { *k == "GIT_TERMINAL_PROMPT" && v.map(|v| v == "0").unwrap_or(false) }));
        assert!(envs.iter().any(|(k, v)| {
            *k == "OMAKURE_GIT_AUTHORITY" && v.map(|v| v == "git.example.test").unwrap_or(false)
        }));
        drop(guard);
        std::env::remove_var("OMAKURE_ASKPASS_ENV");
    }

    #[cfg(unix)]
    #[test]
    fn git_askpass_refuses_credentials_for_another_host() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        std::env::set_var("OMAKURE_ASKPASS_HOST", "host-bound-token");
        let auth = BatteryAuth {
            method: BatteryAuthMethod::HttpsTokenRef,
            token_ref: "secret://env/OMAKURE_ASKPASS_HOST".into(),
        };
        let guard = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
            .unwrap()
            .unwrap();

        let allowed = Command::new(&guard.script_path)
            .arg("Password for 'https://x-access-token@git.example.test':")
            .env("OMAKURE_GIT_AUTHORITY", "git.example.test")
            .output()
            .unwrap();
        let denied = Command::new(&guard.script_path)
            .arg("Password for 'https://x-access-token@internal.example':")
            .env("OMAKURE_GIT_AUTHORITY", "git.example.test")
            .output()
            .unwrap();
        let suffix_denied = Command::new(&guard.script_path)
            .arg("Password for 'https://x-access-token@git.example.test.evil':")
            .env("OMAKURE_GIT_AUTHORITY", "git.example.test")
            .output()
            .unwrap();

        assert!(allowed.status.success());
        assert_eq!(
            String::from_utf8_lossy(&allowed.stdout).trim(),
            "host-bound-token"
        );
        assert!(!denied.status.success());
        assert!(denied.stdout.is_empty());
        assert!(!suffix_denied.status.success());
        assert!(suffix_denied.stdout.is_empty());
        drop(guard);
        std::env::remove_var("OMAKURE_ASKPASS_HOST");
    }

    #[test]
    fn add_battery_records_unsynced_entry_and_rejects_duplicates() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let request = AddBatteryRequest {
            name: "azure".into(),
            git_url: "https://example.invalid/azure.git".into(),
            requested_ref: "main".into(),
            token_ref: None,
        };

        let summary = add_battery(&ws, request.clone()).unwrap();
        let duplicate = add_battery(&ws, request).unwrap_err();

        assert_eq!(summary.name, "azure");
        assert!(summary.resolved_commit.is_none());
        assert_eq!(duplicate.code, OperationErrorCode::AlreadyExists);
    }

    #[test]
    fn add_battery_rejects_invalid_names() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);

        let err = add_battery(
            &ws,
            AddBatteryRequest {
                name: "Azure Scripts".into(),
                git_url: "https://example.invalid/azure.git".into(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::InvalidInput);
    }

    fn run_test_git(args: &[&str], cwd: &Path) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn create_battery_repo() -> TempDir {
        let repo = TempDir::new().unwrap();
        run_test_git(&["init", "-b", "main"], repo.path());
        write_manifest_and_script(repo.path());
        run_test_git(&["add", "."], repo.path());
        run_test_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "initial",
            ],
            repo.path(),
        );
        repo
    }

    #[test]
    fn sync_battery_clones_fetches_detached_commit_and_updates_registry() {
        let repo = create_battery_repo();
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        add_battery(
            &ws,
            AddBatteryRequest {
                name: "azure".into(),
                git_url: repo.path().display().to_string(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();

        let summary = sync_battery(
            &ws,
            SyncBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap();

        assert!(summary.resolved_commit.is_some());
        assert!(ws
            .root()
            .join(summary.cache_path)
            .join(MANIFEST_FILE)
            .exists());
    }

    #[test]
    fn sync_rejects_stale_cache_with_different_origin() {
        let repo_one = create_battery_repo();
        let repo_two = create_battery_repo();
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        add_battery(
            &ws,
            AddBatteryRequest {
                name: "azure".into(),
                git_url: repo_one.path().display().to_string(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();
        sync_battery(
            &ws,
            SyncBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap();
        remove_battery(
            &ws,
            RemoveBatteryRequest {
                name: "azure".into(),
                remove_cache: false,
            },
        )
        .unwrap();
        add_battery(
            &ws,
            AddBatteryRequest {
                name: "azure".into(),
                git_url: repo_two.path().display().to_string(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();

        let err = sync_battery(
            &ws,
            SyncBatteryRequest {
                name: "azure".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::Conflict);
    }

    #[cfg(unix)]
    #[test]
    fn cache_root_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        fs::create_dir_all(paths.cache_root.parent().unwrap()).unwrap();
        let outside = dir.path().join("outside-cache");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &paths.cache_root).unwrap();

        let err = cache_path_for_battery(&ws, "azure").unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[cfg(unix)]
    #[test]
    fn cache_entry_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        fs::create_dir_all(&paths.cache_root).unwrap();
        let outside = dir.path().join("outside-cache-entry");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, paths.cache_root.join("azure")).unwrap();

        let err = cache_path_for_battery(&ws, "azure").unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[cfg(unix)]
    #[test]
    fn registry_write_rejects_symlinked_omakure_parent() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        fs::remove_dir_all(ws.omakure_dir()).unwrap();
        let outside = dir.path().join("outside-omakure");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, ws.omakure_dir()).unwrap();

        let err = write_registry(
            &BatteryPaths::for_workspace(&ws).registry_path,
            &BatteryRegistry::default(),
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn install_battery_script_refuses_overwrite_without_force_and_writes_provenance() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);

        let response = install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: false,
            },
        )
        .unwrap();
        let conflict = install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: false,
            },
        )
        .unwrap_err();

        assert!(response.installed_path.exists());
        assert!(response.provenance_path.exists());
        assert_eq!(conflict.code, OperationErrorCode::Conflict);
    }

    #[test]
    fn provenance_paths_do_not_collide_for_sanitized_script_ids() {
        assert_ne!(
            hex_encode(b"a.b"),
            hex_encode(b"a_b"),
            "hex encoding must preserve distinct script ids"
        );
    }

    #[test]
    fn install_battery_script_does_not_clobber_existing_predictable_temp_sibling() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);
        let old_tmp = ws.scripts_root().join("scripts/list.omakure-install-tmp");
        fs::create_dir_all(old_tmp.parent().unwrap()).unwrap();
        fs::write(&old_tmp, "keep me").unwrap();

        install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: false,
            },
        )
        .unwrap();

        assert_eq!(fs::read_to_string(old_tmp).unwrap(), "keep me");
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_symlinked_parent_directory() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);
        let real = dir.path().join("real-scripts");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, ws.scripts_root().join("scripts")).unwrap();

        let err = install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: false,
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[cfg(unix)]
    #[test]
    fn installed_root_symlink_is_rejected_before_installing_script() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);
        let paths = BatteryPaths::for_workspace(&ws);
        fs::create_dir_all(paths.installed_root.parent().unwrap()).unwrap();
        let outside = dir.path().join("outside-installed");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &paths.installed_root).unwrap();

        let err = install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: false,
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
        assert!(!ws.scripts_root().join("scripts/list.sh").exists());
    }

    #[test]
    fn install_rolls_back_script_when_provenance_write_fails() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);
        let paths = BatteryPaths::for_workspace(&ws);
        let provenance_file = paths
            .installed_root
            .join("azure")
            .join(format!("{}.json", hex_encode(b"azure.list")));
        fs::create_dir_all(&provenance_file).unwrap();

        let err = install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: false,
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::IoFailed);
        assert!(!ws.scripts_root().join("scripts/list.sh").exists());
    }

    #[test]
    fn force_install_restores_existing_script_when_provenance_write_fails() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);
        let target = ws.scripts_root().join("scripts/list.sh");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "old content").unwrap();
        let paths = BatteryPaths::for_workspace(&ws);
        let provenance_file = paths
            .installed_root
            .join("azure")
            .join(format!("{}.json", hex_encode(b"azure.list")));
        fs::create_dir_all(&provenance_file).unwrap();

        let err = install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: true,
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::IoFailed);
        assert_eq!(fs::read_to_string(target).unwrap(), "old content");
    }

    #[test]
    fn force_install_does_not_clobber_existing_backup_sibling() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);
        let target = ws.scripts_root().join("scripts/list.sh");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "old content").unwrap();
        let backup_candidate = target
            .parent()
            .unwrap()
            .join(format!(".list.sh.{}.0.backup", std::process::id()));
        fs::write(&backup_candidate, "keep me").unwrap();

        install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: true,
            },
        )
        .unwrap();

        assert_eq!(fs::read_to_string(backup_candidate).unwrap(), "keep me");
    }

    #[test]
    fn install_battery_script_force_overwrites_existing_target() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_synced_cache_and_registry(&ws);
        let target = ws.scripts_root().join("scripts/list.sh");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "old").unwrap();

        install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: true,
            },
        )
        .unwrap();

        assert!(fs::read_to_string(target)
            .unwrap()
            .contains("OMAKURE_SCHEMA_START"));
    }

    #[test]
    fn remove_battery_unregisters_and_optionally_removes_cache() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let cache = write_synced_cache_and_registry(&ws);

        let response = remove_battery(
            &ws,
            RemoveBatteryRequest {
                name: "azure".into(),
                remove_cache: true,
            },
        )
        .unwrap();

        assert!(response.cache_removed);
        assert!(!cache.exists());
        assert!(list_batteries(&ws).unwrap().is_empty());
    }
}
