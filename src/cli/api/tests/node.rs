use super::*;

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
