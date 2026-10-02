use super::*;

#[cfg(unix)]
#[test]
fn unix_principal_lookup_is_safe_under_concurrency() {
    use std::ffi::CStr;
    use std::ptr;
    use std::sync::{Arc, Barrier};

    const THREADS: usize = 16;
    let euid = unsafe { libc::geteuid() };
    let egid = unsafe { libc::getegid() };
    let (user_name, expected_uid) = {
        let mut entry = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result = ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let status = unsafe {
                libc::getpwuid_r(
                    euid,
                    &mut entry,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if status == libc::ERANGE {
                grow_principal_lookup_buffer(&mut buffer).unwrap();
                continue;
            }
            assert_eq!(status, 0, "resolve current euid");
            assert!(!result.is_null(), "current euid has a passwd entry");
            let name = unsafe { CStr::from_ptr(entry.pw_name) }.to_owned();
            break (name, entry.pw_uid as u32);
        }
    };
    let (group_name, expected_gid) = {
        let mut entry = unsafe { std::mem::zeroed::<libc::group>() };
        let mut result = ptr::null_mut();
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let status = unsafe {
                libc::getgrgid_r(
                    egid,
                    &mut entry,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if status == libc::ERANGE {
                grow_principal_lookup_buffer(&mut buffer).unwrap();
                continue;
            }
            assert_eq!(status, 0, "resolve current egid");
            assert!(!result.is_null(), "current egid has a group entry");
            let name = unsafe { CStr::from_ptr(entry.gr_name) }.to_owned();
            break (name, entry.gr_gid as u32);
        }
    };
    assert_eq!(expected_uid, euid as u32);
    assert_eq!(expected_gid, egid as u32);

    let barrier = Arc::new(Barrier::new(THREADS));
    let handles = (0..THREADS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let user_name = user_name.clone();
            let group_name = group_name.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..256 {
                    let owner = lookup_unix_principal(&user_name, &group_name).unwrap();
                    assert_eq!(owner.uid, expected_uid);
                    assert_eq!(owner.gid, expected_gid);
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[cfg(windows)]
#[test]
fn windows_service_acl_policy_allows_only_required_principals_and_access() {
    const READ: u32 = 0x0012_0089;
    const WRITE: u32 = 0x0012_0116;
    const WRITE_DAC: u32 = 0x0004_0000;
    assert!(windows_security_access_allowed(
        false,
        false,
        true,
        READ | WRITE_DAC
    ));
    assert!(windows_security_access_allowed(
        false,
        true,
        false,
        READ | WRITE
    ));
    assert!(windows_security_access_allowed(false, false, true, READ));
    assert!(windows_security_access_allowed(
        true,
        false,
        true,
        READ | WRITE
    ));
    assert!(!windows_security_access_allowed(
        false,
        false,
        true,
        READ | WRITE
    ));
    assert!(!windows_security_access_allowed(false, false, false, READ));
    assert!(!windows_security_access_allowed(false, true, true, READ));
}

/// Hardening a file must never be an outage.
///
/// 0640 is the *broadest* a public node config may be, not the only mode it
/// may have. An operator who chmods `node.toml` to 0600 has removed access,
/// not granted it, and the node must keep reading it. The previous exact
/// comparison refused 0600 and 0400 alongside 0644, so tightening the
/// config broke the node.
#[cfg(all(unix, debug_assertions))]
#[test]
fn a_public_config_may_be_hardened_but_never_loosened() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("node.toml");
    fs::write(&path, "version = 1\n").unwrap();
    let owner = owner_policy(NodePlatform::Linux, true, false).unwrap();

    // Nothing beyond owner-rw plus group-r may pass. Checked first so a
    // regression reports the widened access, not a stricter-mode edge case.
    for mode in 0..=0o777u32 {
        if mode & !0o640 == 0 {
            continue;
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            validate_file_security(&path, owner, true).is_err(),
            "mode {mode:04o} grants access beyond 0640 and must be refused"
        );
    }

    // Every stricter mode must be readable: hardening is not an outage.
    // 0600 and 0400 are the cases the exact comparison used to refuse.
    for mode in 0..=0o777u32 {
        if mode & !0o640 != 0 {
            continue;
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            validate_file_security(&path, owner, true).is_ok(),
            "mode {mode:04o} is no broader than 0640 and must be accepted"
        );
    }
}

/// The same comparison guards `identity.key`, `authority.key` and
/// `publisher.key` at 0600. Relaxing "exactly" to "no broader than" must
/// not make *those* loosenable.
///
/// Proven exhaustively rather than by sample: all 512 permission modes are
/// checked, and acceptance must hold for exactly the subsets of 0600. That
/// is the whole security argument for using one comparison for both files —
/// no group bit and no other bit can ever pass on a private file.
#[cfg(all(unix, debug_assertions))]
#[test]
fn no_group_or_other_bit_can_ever_pass_on_a_private_file() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let context = test_context(temp.path());
    let path = temp.path().join("identity.key");
    fs::write(&path, b"key").unwrap();

    // The security invariant first, so a regression reports the exposure
    // rather than some stricter-mode edge case that happens to sort lower.
    for mode in 0..=0o777u32 {
        if mode & 0o077 == 0 {
            continue;
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            context.validate_private_file(&path).is_err(),
            "mode {mode:04o} exposes a private key to group or other and must be refused"
        );
    }

    // Then the exact accepted set: the subsets of 0600 and nothing else.
    for mode in 0..=0o777u32 {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(
            context.validate_private_file(&path).is_ok(),
            mode & !0o600 == 0,
            "mode {mode:04o} against a 0600 private file"
        );
    }
}

/// The refusal must lead somewhere. Naming neither the file nor the mode
/// nor the expectation leaves the operator with no route from the error to
/// the fix, and every file this validator guards produced the same
/// sentence.
#[cfg(all(unix, debug_assertions))]
#[test]
fn a_refused_mode_names_the_file_the_mode_and_the_remedy() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("node.toml");
    fs::write(&path, "version = 1\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let owner = owner_policy(NodePlatform::Linux, true, false).unwrap();

    let error = validate_file_security(&path, owner, true).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains(&path.display().to_string()),
        "the refusal must name the file: {message}"
    );
    assert!(
        message.contains("0644"),
        "the refusal must name the mode it found: {message}"
    );
    assert!(
        message.contains("0640"),
        "the refusal must name what it permits: {message}"
    );
    assert!(
        message.contains("chmod 640"),
        "the refusal must carry the remedy: {message}"
    );
}

#[cfg(debug_assertions)]
#[test]
fn symlink_metadata_if_present_treats_missing_paths_as_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let missing = tmp.path().join("node.sqlite-wal");
    assert!(symlink_metadata_if_present(&missing).unwrap().is_none());

    let present = tmp.path().join("present.txt");
    fs::write(&present, b"x").unwrap();
    assert!(symlink_metadata_if_present(&present).unwrap().is_some());
}

#[cfg(debug_assertions)]
#[test]
fn is_not_found_matches_io_not_found_only() {
    let not_found = NodeError::Io(io::Error::new(io::ErrorKind::NotFound, "gone"));
    assert!(is_not_found(&not_found));
    let permission = NodeError::Io(io::Error::new(io::ErrorKind::PermissionDenied, "nope"));
    assert!(!is_not_found(&permission));
    assert!(!is_not_found(&NodeError::InsecurePath("bad".into())));
}
