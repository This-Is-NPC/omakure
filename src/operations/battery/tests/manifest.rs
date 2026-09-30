use super::*;

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
    run_git(&["init", "-b", "main"], &cache);
    run_git(&["add", MANIFEST_FILE, ".gitignore"], &cache);
    run_git(
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
