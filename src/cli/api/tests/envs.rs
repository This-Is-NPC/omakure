use super::*;

#[tokio::test]
async fn env_body_validation_precedes_blocking_capacity() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let envs_dir = workspace.envs_dir().to_path_buf();
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    gate.close();
    let app = super::super::router::router_with_blocking_gate(workspace, gate);

    for body in [r#"{"name": "prod""#, r#"{"params": []}"#] {
        let response = app
            .clone()
            .oneshot(authed_json_request("/v1/envs", body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "invalid_input"
        );
    }

    let response = app
        .oneshot(authed_json_request(
            "/v1/envs",
            r#"{"name":"prod","params":[]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response_json(response).await["error"]["code"], "io_failed");
    assert!(!envs_dir.join("prod.conf").exists());
}

#[tokio::test]
async fn env_endpoints_round_trip_and_redact_values() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let app = router(workspace.clone_for_executor());

    let create = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/envs",
            r#"{"name":"prod","params":[{"key":"HOST","value":"prod.example.com"},{"key":"API_KEY","value":"super_secret"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);

    let show = app
        .clone()
        .oneshot(authed_request("/v1/envs/prod"))
        .await
        .unwrap();
    assert_eq!(show.status(), StatusCode::OK);
    let show_body = response_json(show).await;
    assert_eq!(show_body["data"][0]["key"], "HOST");
    assert_eq!(show_body["data"][1]["key"], "API_KEY");
    assert_eq!(show_body["data"][1]["value"], "****");
    assert!(!show_body.to_string().contains("super_secret"));

    let set = app
        .clone()
        .oneshot(authed_json_method_request(
            Method::PUT,
            "/v1/envs/prod/params/PORT",
            r#"{"value":"443"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(set.status(), StatusCode::OK);

    let activate = app
        .clone()
        .oneshot(authed_json_request("/v1/envs/prod/activate", r#"{}"#))
        .await
        .unwrap();
    assert_eq!(activate.status(), StatusCode::OK);

    let list = app
        .clone()
        .oneshot(authed_request("/v1/envs"))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = response_json(list).await;
    assert_eq!(list_body["data"][0]["name"], "prod");
    assert_eq!(list_body["data"][0]["file"], "prod.conf");
    assert_eq!(list_body["data"][0]["active"], true);

    let remove_param = app
        .clone()
        .oneshot(authed_delete_request("/v1/envs/prod/params/API_KEY"))
        .await
        .unwrap();
    assert_eq!(remove_param.status(), StatusCode::OK);

    let deactivate = app
        .clone()
        .oneshot(authed_delete_request("/v1/envs/active"))
        .await
        .unwrap();
    assert_eq!(deactivate.status(), StatusCode::OK);

    let delete = app
        .oneshot(authed_delete_request("/v1/envs/prod"))
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);
    assert!(!workspace.envs_dir().join("prod.conf").exists());
}

#[tokio::test]
async fn env_endpoints_replace_patch_and_reject_conf_route_names() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let app = router(workspace);

    let replace = app
        .clone()
        .oneshot(authed_json_method_request(
            Method::PUT,
            "/v1/envs/dev",
            r#"{"params":[{"key":"HOST","value":"localhost"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(replace.status(), StatusCode::OK);

    let patch = app
        .clone()
        .oneshot(authed_json_method_request(
            Method::PATCH,
            "/v1/envs/dev",
            r#"{"params":[{"key":"HOST","value":"127.0.0.1"},{"key":"TOKEN","value":"secret_value"}]}"#,
        ))
        .await
        .unwrap();
    assert_eq!(patch.status(), StatusCode::OK);

    let show = app
        .clone()
        .oneshot(authed_request("/v1/envs/dev"))
        .await
        .unwrap();
    let show_body = response_json(show).await;
    assert_eq!(show_body["data"][0]["value"], "127.0.0.1");
    assert_eq!(show_body["data"][1]["value"], "****");
    assert!(!show_body.to_string().contains("secret_value"));

    let invalid = app
        .oneshot(authed_request("/v1/envs/dev.conf"))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_body = response_json(invalid).await;
    assert_eq!(invalid_body["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn env_endpoints_require_specific_policy_capabilities() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    env_ops::create_env(
        &workspace,
        "prod",
        &[env_ops::EnvParam {
            key: "HOST".into(),
            value: "prod.example.com".into(),
        }],
    )
    .unwrap();
    let app = router_with_policy(
        test_credential::authenticator(&["envs:read"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );

    let read = app
        .clone()
        .oneshot(authed_request("/v1/envs/prod"))
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);

    for request in [
        authed_json_method_request(
            Method::PATCH,
            "/v1/envs/prod",
            r#"{"params":[{"key":"HOST","value":"changed"}]}"#,
        ),
        authed_json_request("/v1/envs/prod/activate", r#"{}"#),
        authed_delete_request("/v1/envs/active"),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
    }
}
