use super::*;

#[tokio::test]
async fn health_works_without_token() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let response = router(workspace)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["status"], "ok");
}

#[tokio::test]
async fn ready_works_without_token_when_no_gate() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let response = router(workspace)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["status"], "ready");
    let data = body["data"].as_object().expect("data object");
    assert_eq!(data.keys().collect::<Vec<_>>(), vec!["status"]);
}

#[tokio::test]
async fn ready_returns_503_when_gate_not_ready() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let gate = ReadinessGate::new(true, false, true, false);
    let app = router_with_policy(
        test_credential::authenticator(&["*"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        Some(gate),
        BODY_LIMIT_BYTES,
    );
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(response).await;
    assert_eq!(body["data"]["status"], "not_ready");
}

#[tokio::test]
async fn admin_status_requires_scope_and_exposes_reload_without_secrets() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let admin_plain = auth::test_token_plaintext("ops-admin");
    let admin_hash = auth::hash_token(&admin_plain).unwrap();
    let reader_plain = auth::test_token_plaintext("reader");
    let reader_hash = auth::hash_token(&reader_plain).unwrap();
    let tokens_path = dir.path().join("tokens.toml");
    std::fs::write(
        &tokens_path,
        format!(
            r#"
version = 1
[[tokens]]
id = "ops-admin"
hash = "{admin_hash}"
scopes = ["admin:status"]
enabled = true
[[tokens]]
id = "reader"
hash = "{reader_hash}"
scopes = ["scripts:read"]
enabled = true
"#
        ),
    )
    .unwrap();
    let auth = Authenticator::from_tokens_file(&tokens_path).unwrap();
    let gate = ReadinessGate::new(true, false, true, false);
    gate.set_workers_alive(true);
    let app = router_with_policy(
        auth.clone(),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        Some(gate),
        BODY_LIMIT_BYTES,
    );

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/admin/status")
                .header(header::AUTHORIZATION, format!("Bearer {reader_plain}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let ok = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/admin/status")
                .header(header::AUTHORIZATION, format!("Bearer {admin_plain}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = response_json(ok).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["ready"], true);
    assert_eq!(body["data"]["auth"]["mode"], "tokens_file");
    assert_eq!(body["data"]["auth"]["token_count"], 2);
    let serialized = body.to_string();
    assert!(!serialized.contains(&admin_plain));
    assert!(!serialized.contains(&reader_plain));
    assert!(!serialized.contains("ops-admin"));
    assert!(!serialized.contains("reader"));
    assert!(!serialized.contains(tokens_path.to_string_lossy().as_ref()));

    // Failed reload keeps last valid set and surfaces status.
    std::fs::write(&tokens_path, "not-valid-toml [[[").unwrap();
    assert!(auth.reload().is_err());
    let after = auth.status();
    assert_eq!(after.last_reload_ok, Some(false));
    assert!(auth.authenticate(&admin_plain).is_some());
}

#[tokio::test]
async fn ready_remains_minimal_without_token_ids_after_admin_exists() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let gate = ReadinessGate::new(false, false, false, false);
    let app = router_with_policy(
        test_credential::authenticator(&["*"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        Some(gate),
        BODY_LIMIT_BYTES,
    );
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let data = body["data"].as_object().expect("data object");
    assert_eq!(data.keys().collect::<Vec<_>>(), vec!["status"]);
    assert!(!body.to_string().contains("token"));
}

#[tokio::test]
async fn workspace_endpoint_returns_summary() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_request("/v1/workspace"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["version"], app_meta::APP_VERSION);
}

#[tokio::test]
async fn config_endpoint_returns_full_masked_config() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(workspace.envs_dir().join("dev.conf"), "HOST=localhost\n").unwrap();
    std::fs::write(workspace.envs_active_path(), "dev.conf\n").unwrap();

    let response = router(workspace)
        .oneshot(authed_request("/v1/config"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["version"], app_meta::APP_VERSION);
    assert_eq!(body["data"]["active_env"], "dev.conf");
    assert_eq!(body["data"]["active_env_keys"][0]["key"], "HOST");
    assert_eq!(body["data"]["active_env_keys"][0]["value"], "****");
    assert!(!body.to_string().contains("localhost"));
}

#[tokio::test]
async fn doctor_endpoint_returns_structured_report() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");

    let response = router(workspace)
        .oneshot(authed_request("/v1/doctor"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert!(body["data"]["dependencies"].is_array());
    assert!(body["data"]["workspace_paths"].is_array());
    assert_eq!(body["data"]["schemas"]["total"], 1);
}
