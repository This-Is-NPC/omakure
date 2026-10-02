use super::*;

#[test]
fn bind_guard_allows_loopback_by_default() {
    let addr: SocketAddr = "127.0.0.1:7878".parse().unwrap();
    assert!(validate_bind(addr, false).is_ok());
}

#[test]
fn bind_guard_rejects_non_loopback_without_opt_in() {
    let addr: SocketAddr = "0.0.0.0:7878".parse().unwrap();
    assert_eq!(
        validate_bind(addr, false),
        Err(ApiConfigError::NonLoopbackBind(addr))
    );
}

#[test]
fn bind_guard_allows_non_loopback_with_opt_in() {
    let addr: SocketAddr = "0.0.0.0:7878".parse().unwrap();
    assert!(validate_bind(addr, true).is_ok());
}

#[test]
fn prepare_api_boot_rejects_bad_policy_before_bind() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bad-policy.toml");
    std::fs::write(&path, "version = 1\nnot = [valid\n").unwrap();
    let args = ApiArgs {
        bind: "127.0.0.1:7878".parse().unwrap(),
        allow_non_loopback: false,
        policy: Some(path),
        tokens_file: None,
        secret_refs: vec![],
    };
    let err = match prepare_api_boot(&args) {
        Err(e) => e,
        Ok(_) => panic!("expected policy parse failure"),
    };
    assert!(matches!(err, ApiConfigError::Policy(_)), "got {err}");
    assert!(err.to_string().contains("parse"));
}

#[test]
fn prepare_api_boot_requires_a_tokens_file() {
    let args = ApiArgs {
        bind: "127.0.0.1:7878".parse().unwrap(),
        allow_non_loopback: false,
        policy: None,
        tokens_file: None,
        secret_refs: vec![],
    };
    let err = match prepare_api_boot(&args) {
        Err(e) => e,
        Ok(_) => panic!("expected auth failure without tokens file"),
    };
    assert!(matches!(err, ApiConfigError::Auth(_)), "got {err}");
    assert!(err.to_string().contains("--tokens-file"));
}

#[test]
fn prepare_api_boot_policy_allow_non_loopback_permits_bind() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("policy.toml");
    std::fs::write(
        &path,
        r#"
version = 1
[http]
allow_non_loopback = true
bind = "0.0.0.0:7878"
"#,
    )
    .unwrap();
    let tokens = dir.path().join("tokens.toml");
    let plaintext = auth::test_token_plaintext("admin");
    let hash = auth::hash_token(&plaintext).unwrap();
    std::fs::write(
        &tokens,
        format!(
            r#"
version = 1
[[tokens]]
id = "admin"
hash = "{hash}"
scopes = ["*"]
enabled = true
"#
        ),
    )
    .unwrap();
    let args = ApiArgs {
        bind: "127.0.0.1:7878".parse().unwrap(),
        allow_non_loopback: false,
        policy: Some(path),
        tokens_file: Some(tokens),
        secret_refs: vec![],
    };
    let boot = prepare_api_boot(&args).unwrap();
    assert_eq!(boot.bind.to_string(), "0.0.0.0:7878");
}
