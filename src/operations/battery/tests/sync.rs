use super::*;

#[test]
fn token_resolution_preserves_forbidden_mapping_for_typed_secret_errors() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace_in(&temp);
    let access = SecretAccess::allow_all();

    let invalid =
        super::super::sync::resolve_battery_token(&workspace, "secret://", &access).unwrap_err();
    assert_eq!(invalid.code, OperationErrorCode::Forbidden);
    assert_eq!(
        invalid.message,
        "failed to resolve battery token_ref: invalid secret ref"
    );

    let missing =
        super::super::sync::resolve_battery_token(&workspace, "secret://prod/absent", &access)
            .unwrap_err();
    assert_eq!(missing.code, OperationErrorCode::Forbidden);
    assert_eq!(
        missing.message,
        "failed to resolve battery token_ref: secret ref not found"
    );
}

#[test]
fn sync_battery_is_idempotent_on_repeated_prepare_and_sync() {
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

    let first = sync_battery(
        &ws,
        SyncBatteryRequest {
            name: "azure".into(),
        },
    )
    .unwrap();
    let second = sync_battery(
        &ws,
        SyncBatteryRequest {
            name: "azure".into(),
        },
    )
    .unwrap();

    assert_eq!(second.resolved_commit, first.resolved_commit);
    assert!(
        ws.root()
            .join(second.cache_path)
            .join(MANIFEST_FILE)
            .exists()
    );
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
