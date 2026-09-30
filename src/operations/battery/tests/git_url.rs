use super::*;

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
        strip_windows_verbatim_owned(repo.canonicalize().unwrap().display().to_string())
    );
    assert!(Path::new(&normalized).is_absolute());
}

#[cfg(windows)]
#[test]
fn local_git_source_normalizes_verbatim_input_for_git() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let verbatim = format!(r"\\?\{}", repo.display());
    let normalized = normalize_git_url(&verbatim).unwrap();

    assert_eq!(
        normalized,
        strip_windows_verbatim_owned(repo.canonicalize().unwrap().display().to_string())
    );
    assert!(!normalized.starts_with(r"\\?\"));
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
