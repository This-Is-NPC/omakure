use super::*;

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
