use super::*;

#[test]
fn install_request_keeps_force_explicit() {
    let request = InstallBatteryScriptRequest {
        battery_name: "azure".into(),
        script_id: "azure.rg-list-all".into(),
        force: false,
    };

    assert!(!request.force);
}

#[cfg(unix)]
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
#[cfg(unix)]
fn install_keeps_the_source_execute_bits_and_owner_only_write() {
    use std::os::unix::fs::PermissionsExt;

    for (source_mode, installed_mode) in [(0o775, 0o755), (0o644, 0o644), (0o700, 0o700)] {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        let paths = BatteryPaths::for_workspace(&ws);
        let cache = paths.cache_path_for("azure");
        write_manifest_and_script(&cache);
        fs::set_permissions(
            cache.join("scripts/list.sh"),
            fs::Permissions::from_mode(source_mode),
        )
        .unwrap();
        let commit = init_cache_git(&cache);
        write_registry(&paths.registry_path, &synced_registry_with_commit(commit)).unwrap();

        let response = install_battery_script(
            &ws,
            InstallBatteryScriptRequest {
                battery_name: "azure".into(),
                script_id: "azure.list".into(),
                force: false,
            },
        )
        .unwrap();

        let mode = fs::metadata(&response.installed_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, installed_mode,
            "a {source_mode:o} source must install as {installed_mode:o}"
        );
    }
}

#[test]
fn provenance_paths_do_not_collide_for_sanitized_script_ids() {
    assert_ne!(
        hex::encode(b"a.b"),
        hex::encode(b"a_b"),
        "hex encoding must preserve distinct script ids"
    );
}

#[test]
#[cfg(unix)]
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
fn verified_install_rejects_symlinked_parent_before_writing() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, ws.scripts_root().join("scripts")).unwrap();

    let error = install_verified_script(&ws, Path::new("scripts/list.sh"), b"echo safe\n", 0o755)
        .err()
        .expect("a symlinked install parent must fail");
    assert_eq!(error.code, OperationErrorCode::UnsafePath);
    assert!(!outside.join("list.sh").exists());
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
#[cfg(unix)]
fn install_rolls_back_script_when_provenance_write_fails() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_synced_cache_and_registry(&ws);
    let paths = BatteryPaths::for_workspace(&ws);
    let provenance_file = paths
        .installed_root
        .join("azure")
        .join(format!("{}.json", hex::encode(b"azure.list")));
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
#[cfg(unix)]
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
        .join(format!("{}.json", hex::encode(b"azure.list")));
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
#[cfg(unix)]
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
#[cfg(unix)]
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
