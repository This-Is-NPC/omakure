use super::*;

#[cfg(unix)]
#[test]
fn opened_file_identity_detects_path_replacement() {
    let tmp = tempfile::TempDir::new().unwrap();
    let opened_path = tmp.path().join("opened.toml");
    let replacement_path = tmp.path().join("replacement.toml");
    fs::write(&opened_path, "opened").unwrap();
    fs::write(&replacement_path, "replacement").unwrap();
    let file = fs::File::open(&opened_path).unwrap();
    assert!(validate_open_file_identity(&opened_path, &file).is_ok());
    assert!(validate_open_file_identity(&replacement_path, &file).is_err());
}

#[cfg(all(unix, debug_assertions))]
#[test]
fn private_bounded_file_reader_enforces_path_mode_and_size() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let tmp = tempfile::TempDir::new().unwrap();
    let context = test_context(tmp.path());
    let token = tmp.path().join("bootstrap.token");
    fs::write(&token, b"bounded").unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let lease = context.stage_private_bounded_file(&token, 7).unwrap();
    assert_eq!(lease.contents(), b"bounded");
    lease.restore().unwrap();
    assert!(context.stage_private_bounded_file(&token, 6).is_err());
    fs::set_permissions(&token, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(context.stage_private_bounded_file(&token, 7).is_err());
    fs::remove_file(&token).unwrap();

    let target = tmp.path().join("real.token");
    fs::write(&target, b"secret").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, &token).unwrap();
    assert!(context.stage_private_bounded_file(&token, 6).is_err());
}

#[cfg(all(unix, debug_assertions))]
#[test]
fn startup_tombstone_scan_is_bounded() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().unwrap();
    let context = test_context(tmp.path());
    let token = tmp.path().join("bootstrap.token");
    for index in 0..11 {
        let tombstone = tmp.path().join(format!(
            "{PRIVATE_TOKEN_TOMBSTONE_PREFIX}{index:032x}-bootstrap.token"
        ));
        fs::write(&tombstone, b"t").unwrap();
        fs::set_permissions(&tombstone, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let tombstones = context.list_private_token_tombstones(&token, 1).unwrap();
    assert_eq!(tombstones.len(), PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT);
    for tombstone in tombstones {
        fs::remove_file(tombstone.tombstone_path).unwrap();
    }
    for entry in fs::read_dir(tmp.path()).unwrap() {
        let entry = entry.unwrap();
        fs::remove_file(entry.path()).unwrap();
    }
}
