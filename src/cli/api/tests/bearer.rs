use super::*;

#[tokio::test]
async fn protected_route_rejects_missing_token() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let response = router(workspace)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/scripts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response_json(response).await;
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn protected_route_rejects_invalid_token() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let response = router(workspace)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/scripts")
                .header(header::AUTHORIZATION, "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn protected_route_accepts_valid_token() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let response = router(workspace)
        .oneshot(authed_request("/v1/unknown"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn tokens_file_unknown_token_is_401_missing_scope_is_403() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");

    let plaintext = auth::test_token_plaintext("reader");
    let hash = auth::hash_token(&plaintext).unwrap();
    let path = dir.path().join("tokens.toml");
    std::fs::write(
        &path,
        format!(
            r#"
version = 1
[[tokens]]
id = "reader"
hash = "{hash}"
scopes = ["scripts:read"]
enabled = true
"#
        ),
    )
    .unwrap();
    let auth = Authenticator::from_tokens_file(&path).unwrap();
    // Process-wide capabilities would allow runs:write; file mode must ignore them.
    let app = router_with_auth(auth, workspace, ApiPolicy::default());

    let unknown = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/scripts")
                .header(
                    header::AUTHORIZATION,
                    "Bearer omk_live_not_a_real_token_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);

    let forbidden = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/runs")
                .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    let body = response_json(forbidden).await;
    assert_eq!(body["error"]["code"], "forbidden");

    let allowed = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/scripts")
                .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn tokens_file_env_scope_alias_envs_read() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let plaintext = auth::test_token_plaintext("env-reader");
    let hash = auth::hash_token(&plaintext).unwrap();
    let path = dir.path().join("tokens.toml");
    std::fs::write(
        &path,
        format!(
            r#"
version = 1
[[tokens]]
id = "env-reader"
hash = "{hash}"
scopes = ["envs:read"]
enabled = true
"#
        ),
    )
    .unwrap();
    let app = router_with_auth(
        Authenticator::from_tokens_file(&path).unwrap(),
        workspace,
        ApiPolicy::default(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/envs")
                .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_plane_router_gates_reads_on_node_read_capability() {
    let registry = shared_test_health_registry();
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let deploy = DeployPolicy::default();
    let denied = router_with_health_plane(
        test_credential::authenticator(&["node:write"]),
        workspace.clone_for_executor(),
        ApiPolicy::default(),
        deploy.clone(),
        None,
        registry,
        BODY_LIMIT_BYTES,
    )
    .oneshot(authed_request("/v1/node/health"))
    .await
    .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let allowed = router_with_health_plane(
        test_credential::authenticator(&["node:read"]),
        workspace,
        ApiPolicy::default(),
        deploy,
        None,
        shared_test_health_registry(),
        BODY_LIMIT_BYTES,
    )
    .oneshot(authed_request("/v1/node/health"))
    .await
    .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_authentications_recycle_permits_and_do_not_deadlock() {
    // Fire more concurrent requests than the bounded auth budget. Requests
    // either authenticate or fail fast; none may queue indefinitely.
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let app = router(workspace);
    let mut handles = Vec::new();
    for _ in 0..24 {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            app.oneshot(authed_request("/v1/config"))
                .await
                .unwrap()
                .status()
        }));
    }
    let mut accepted = 0;
    for handle in handles {
        match handle.await.unwrap() {
            StatusCode::OK => accepted += 1,
            StatusCode::SERVICE_UNAVAILABLE => {}
            status => panic!("unexpected auth response: {status}"),
        }
    }
    assert!(accepted > 0);
}
