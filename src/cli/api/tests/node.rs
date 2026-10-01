use super::*;

#[tokio::test(flavor = "current_thread")]
async fn node_routes_use_the_shared_blocking_gate_after_validation() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    gate.close();
    let app = super::super::router::router_with_blocking_gate(workspace, gate);

    for path in ["/v1/node/status", "/v1/node/peers", "/v1/node/enrollments"] {
        let response = app.clone().oneshot(authed_request(path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response_json(response).await["error"]["code"], "io_failed");
    }

    let malformed = app
        .clone()
        .oneshot(authed_json_request("/v1/node/init", "{"))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    assert_eq!(
        app.oneshot(authed_request("/v1/health"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn discovery_status_requires_its_explicit_scope_and_redacts_addresses() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let denied = router_with_policy(
        test_credential::authenticator(&["node:read"]),
        workspace.clone_for_executor(),
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    )
    .oneshot(authed_request("/v1/node/discovery?include_addresses=true"))
    .await
    .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let allowed = router_with_policy(
        test_credential::authenticator(&["discovery:read"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    )
    .oneshot(authed_request("/v1/node/discovery?include_addresses=true"))
    .await
    .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let body = response_json(allowed).await;
    assert_eq!(body["data"]["candidate_count"], 0);
    assert_eq!(body["data"]["candidates"], serde_json::json!([]));
}
