use super::*;

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
    run_git(&["config", "credential.helper", "!/bin/false"], &cache);

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
