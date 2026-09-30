use super::*;

#[tokio::test]
async fn secrets_metadata_endpoint_redacts_values() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let secret_value = "metadata-must-not-leak-this-value";
    std::fs::write(
        workspace.envs_dir().join("prod.conf"),
        format!("TOKEN={secret_value}\n"),
    )
    .unwrap();
    let mut deploy = DeployPolicy::default();
    deploy.secrets.metadata_endpoint = true;
    let app = router_with_deploy(
        test_credential::authenticator(&["secrets:read-metadata"]),
        workspace,
        ApiPolicy::with_secret_refs(["secret://prod/*"]),
        deploy,
    );
    let response = app.oneshot(authed_request("/v1/secrets")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let rendered = body.to_string();
    assert!(!rendered.contains(secret_value));
    assert_eq!(body["data"][0]["id"], "secret://prod/token");
    assert!(body["data"][0]["source"]
        .as_str()
        .unwrap()
        .starts_with("file:"));
    assert!(body["data"][0].get("value").is_none());
}

#[tokio::test]
async fn secrets_metadata_requires_scope_and_policy_flag() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let mut deploy = DeployPolicy::default();
    deploy.secrets.metadata_endpoint = false;
    let disabled = router_with_deploy(
        test_credential::authenticator(&["secrets:read-metadata"]),
        workspace.clone_for_executor(),
        ApiPolicy::default(),
        deploy.clone(),
    );
    let response = disabled
        .oneshot(authed_request("/v1/secrets"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    deploy.secrets.metadata_endpoint = true;
    let denied = router_with_deploy(
        test_credential::authenticator(&["batteries:read"]),
        workspace,
        ApiPolicy::default(),
        deploy,
    );
    let response = denied.oneshot(authed_request("/v1/secrets")).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
