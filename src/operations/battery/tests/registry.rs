use super::*;

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
    let registry_text = fs::read_to_string(BatteryPaths::for_workspace(&ws).registry_path).unwrap();
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
