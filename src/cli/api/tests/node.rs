use super::*;
use std::time::Duration;

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

#[tokio::test(flavor = "current_thread")]
async fn queued_node_delivery_preflight_keeps_health_available() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let app = super::super::router::router_with_blocking_gate(workspace, Arc::clone(&gate));
    let requests = [
        (
            "/v1/node/cues",
            r#"{"peer_node_id":"peer","script":"deploy.sh","reason":"test"}"#,
            "no direct transport is running, so there is no session to carry a cue",
        ),
        (
            "/v1/node/baselines",
            r#"{"peer_node_id":"peer","manifest":"00","scripts":[]}"#,
            "no direct transport is running, so there is no session to carry a baseline",
        ),
    ];

    for (path, body, expected_error) in requests {
        let permit = Arc::clone(&gate).acquire_owned().await.unwrap();
        let request_app = app.clone();
        let pending = tokio::spawn(async move {
            request_app
                .oneshot(authed_json_request(path, body))
                .await
                .unwrap()
        });
        let health = tokio::time::timeout(
            Duration::from_secs(1),
            app.clone().oneshot(authed_request("/v1/health")),
        )
        .await
        .expect("health must progress while delivery waits")
        .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert!(
            !pending.is_finished(),
            "delivery must wait for the blocking gate"
        );

        drop(permit);
        let response = tokio::time::timeout(Duration::from_secs(3), pending)
            .await
            .expect("delivery must finish after permit release")
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["message"], expected_error);
    }

    gate.close();
    for (path, body, _) in requests {
        let response = app
            .clone()
            .oneshot(authed_json_request(path, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response_json(response).await["error"]["code"], "io_failed");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn health_plane_reads_use_a_bounded_blocking_gate() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let deploy = DeployPolicy::default();
    let auth = test_credential::authenticator(&["node:read"]);
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    gate.close();
    let health = super::super::router::health_plane_router_with_blocking_gate(
        shared_test_health_registry(),
        auth.clone(),
        deploy.clone(),
        super::super::boot::auth_verification_gate(&deploy),
        gate,
        BODY_LIMIT_BYTES,
    );
    let app = router_with_policy(
        auth,
        workspace,
        ApiPolicy::default(),
        deploy,
        None,
        BODY_LIMIT_BYTES,
    )
    .nest("/v1/node", health);

    for path in ["/v1/node/health", "/v1/node/signals"] {
        let response = app.clone().oneshot(authed_request(path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response_json(response).await["error"]["code"], "io_failed");
    }

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
