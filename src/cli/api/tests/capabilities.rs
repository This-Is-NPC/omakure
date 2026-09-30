use super::*;

#[test]
fn wildcard_scope_does_not_bypass_secret_refs() {
    let wildcard = AuthContext {
        token_id: "admin".into(),
        scopes: vec!["*".into()],
    };
    let access = ApiPolicy::from_secret_refs(&[]).secret_access(&wildcard);
    assert!(crate::secrets::check_secret_access("secret://prod/token", &access).is_err());

    let access = ApiPolicy::from_secret_refs(&["*".into()]).secret_access(&wildcard);
    assert!(crate::secrets::check_secret_access("secret://prod/token", &access).is_ok());
}

#[tokio::test]
async fn read_endpoints_require_explicit_read_capabilities() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let app = router_with_policy(
        test_credential::authenticator(&["runs:write"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );

    for uri in [
        "/v1/config",
        "/v1/workspace",
        "/v1/doctor",
        "/v1/scripts",
        "/v1/tree",
        "/v1/runs",
        "/v1/queue/stats",
        "/v1/batteries",
    ] {
        let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "uri: {uri}");
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
    }
}

#[tokio::test]
async fn read_endpoints_accept_matching_read_capabilities() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let app = router_with_policy(
        test_credential::authenticator(&[
            "config:read",
            "scripts:read",
            "runs:read",
            "batteries:read",
        ]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );

    for uri in [
        "/v1/config",
        "/v1/workspace",
        "/v1/doctor",
        "/v1/scripts",
        "/v1/tree",
        "/v1/runs",
        "/v1/queue/stats",
        "/v1/batteries",
    ] {
        let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "uri: {uri}");
    }
}

#[tokio::test]
async fn deploy_policy_writes_false_forbids_writes_even_with_wildcard_token() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let mut deploy = DeployPolicy::default();
    deploy.routes.writes = false;
    let app = router_with_deploy(
        test_credential::authenticator(&["*"]),
        workspace,
        ApiPolicy::default(),
        deploy,
    );

    let read = app
        .clone()
        .oneshot(authed_request("/v1/scripts"))
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);

    let write = app
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"job.sh","run_id":"policy-ro"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(write.status(), StatusCode::FORBIDDEN);
    let body = response_json(write).await;
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("deployment policy"));
}

#[tokio::test]
async fn deploy_policy_battery_false_forbids_all_battery_routes() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let mut deploy = DeployPolicy::default();
    deploy.routes.battery = false;
    let app = router_with_deploy(
        test_credential::authenticator(&["*"]),
        workspace,
        ApiPolicy::default(),
        deploy,
    );

    for uri in [
        "/v1/batteries",
        "/v1/batteries/azure",
        "/v1/batteries/azure/scripts",
    ] {
        let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "uri={uri}");
    }
    let sync = app
        .oneshot(authed_json_request("/v1/batteries/azure/sync", r#"{}"#))
        .await
        .unwrap();
    assert_eq!(sync.status(), StatusCode::FORBIDDEN);
}
