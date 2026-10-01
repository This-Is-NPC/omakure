use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::git::{
    assert_git_http_pinning_supported, git_checkout_detached_spec, git_clone_spec, git_fetch_spec,
    prepare_git_askpass, prepare_git_config, reject_unsafe_local_git_config, run_git_capture,
    run_git_capture_with_context, run_git_with_context, GitCommandSpec, GitExecContext,
    GitTransportPolicy,
};
use super::git_url::{resolve_public_git_endpoint, validate_git_ref, validate_git_url};
use super::manifest::{load_manifest, validate_manifest_for_battery};
use super::registry::{
    cache_path_for_battery, read_registry, validate_battery_name, write_registry, BatteryPaths,
};
use super::types::{BatterySummary, SyncBatteryRequest};
use crate::secrets::{self, SecretAccess};
use crate::workspace::Workspace;
use std::fs;
use std::path::Path;

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
    let git_config = prepare_git_config(workspace)?;
    let git_ctx = GitExecContext {
        policy,
        askpass: askpass.as_ref(),
        http_pin: http_pin.as_ref(),
        global_config: Some(&git_config),
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
        if summary.auth.is_some() {
            let registry_text = fs::read_to_string(&paths.registry_path).unwrap_or_default();
            if askpass
                .as_ref()
                .map(|askpass| askpass.token.as_str())
                .is_some_and(|token| !token.is_empty() && registry_text.contains(token))
            {
                return Err(OperationError::new(
                    OperationErrorCode::Conflict,
                    "refusing to persist resolved battery credentials",
                ));
            }
        }
        Ok(summary)
    })();
    drop(askpass);
    sync_result
}

pub(super) fn resolve_battery_token(
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

pub(super) fn verify_synced_checkout(
    cache_path: &Path,
    expected_commit: &str,
) -> OperationResult<()> {
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
