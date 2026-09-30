use super::*;

#[tokio::test]
async fn authenticated_mutating_request_emits_audit_with_token_id_redacted_auth() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let plaintext = auth::test_token_plaintext("ci-enqueue");
    let hash = auth::hash_token(&plaintext).unwrap();
    let tokens_path = dir.path().join("tokens.toml");
    std::fs::write(
        &tokens_path,
        format!(
            r#"
version = 1
[[tokens]]
id = "ci-enqueue"
hash = "{hash}"
scopes = ["runs:enqueue", "runs:read"]
enabled = true
"#
        ),
    )
    .unwrap();
    let sink = AuditCapture::install().await;
    let app = router_with_policy(
        Authenticator::from_tokens_file(&tokens_path).unwrap(),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );
    let request_body = r#"{"script":"job.sh","run_id":"rid-audit","actor":"agent","reason":"request-secret-marker"}"#;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/runs")
                .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(request_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let events = sink.events();
    let event = events
        .iter()
        .find(|e| e.run_id.as_deref() == Some("rid-audit"))
        .expect("audit event for POST /v1/runs");
    assert_eq!(event.token_id.as_deref(), Some("ci-enqueue"));
    assert_eq!(event.run_id.as_deref(), Some("rid-audit"));
    assert_eq!(event.outcome, "ok");
    assert_eq!(event.status, 200);
    let serialized = serde_json::to_string(event).unwrap();
    assert!(!serialized.contains(&plaintext));
    assert!(!serialized.contains("Authorization"));
    assert!(!serialized.to_lowercase().contains("bearer "));
    assert!(!serialized.contains("request-secret-marker"));

    let duplicate = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/runs")
                .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(request_body))
                .unwrap(),
        )
        .await
        .unwrap();
    let duplicate_status = duplicate.status();
    assert!(!duplicate_status.is_success());
    assert!(sink.events().iter().any(|event| {
        event.run_id.as_deref() == Some("rid-audit") && event.status == duplicate_status.as_u16()
    }));
}

#[tokio::test]
async fn rejected_enqueue_audit_keeps_safe_run_id_without_request_secrets() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_secret_script(workspace.scripts_root(), "secret.sh", None);
    let mut deploy = DeployPolicy::default();
    deploy.runs.allow_secret_fields = false;
    let sink = AuditCapture::install().await;
    let app = router_with_policy(
        test_credential::authenticator(&["*"]),
        workspace,
        ApiPolicy::default(),
        deploy,
        None,
        BODY_LIMIT_BYTES,
    );

    let response = app
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret.sh","run_id":"rid-denied-audit","args":["--token","secret://request-secret-marker/token"]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let event = sink
        .events()
        .into_iter()
        .find(|event| event.run_id.as_deref() == Some("rid-denied-audit"))
        .expect("audit event for rejected enqueue");
    assert_eq!(event.run_id.as_deref(), Some("rid-denied-audit"));
    let serialized = serde_json::to_string(&event).unwrap();
    assert!(!serialized.contains("request-secret-marker"));
    assert!(!serialized.contains(test_credential::token()));
}

#[tokio::test]
async fn unauthorized_request_emits_audit_without_token_id() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let sink = AuditCapture::install().await;
    let app = router(workspace);
    let response = app
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
    let events = sink.events();
    let event = events
        .iter()
        .find(|e| e.path == "/v1/scripts" && e.outcome == "unauthorized")
        .expect("401 audit event");
    assert_eq!(event.token_id, None);
    assert_eq!(event.run_id, None);
    assert_eq!(event.status, 401);
    let serialized = serde_json::to_string(event).unwrap();
    assert!(!serialized.contains("wrong-token"));
    assert!(!serialized.contains("Authorization"));
}
