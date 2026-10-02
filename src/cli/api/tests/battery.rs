use super::*;

#[tokio::test]
async fn battery_registration_checks_security_before_blocking_capacity() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    gate.close();
    let app = super::super::router::router_with_blocking_gate(workspace, gate);

    let invalid_url = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"bad","git_url":"http://example.com/repo.git"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_url.status(), StatusCode::BAD_REQUEST);

    let private_auth = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"private","git_url":"https://example.com/repo.git","token_ref":"secret://prod/token"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(private_auth.status(), StatusCode::FORBIDDEN);

    let unavailable = app
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"public","git_url":"https://example.com/repo.git"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response_json(unavailable).await["error"]["code"],
        "io_failed"
    );
}

#[tokio::test]
async fn mutating_battery_routes_require_battery_write_capability() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    battery_ops::add_battery(
        &workspace,
        battery_ops::AddBatteryRequest {
            name: "azure".into(),
            git_url: "https://example.invalid/azure.git".into(),
            requested_ref: "main".into(),
            token_ref: None,
        },
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

    for request in [
        authed_json_request(
            "/v1/batteries",
            r#"{"name":"new","git_url":"https://example.invalid/new.git"}"#,
        ),
        authed_json_request("/v1/batteries/azure/sync", r#"{}"#),
        authed_json_request("/v1/batteries/azure/scripts/list/install", r#"{}"#),
        authed_delete_request("/v1/batteries/azure"),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
    }
}

#[tokio::test]
async fn battery_add_with_token_ref_stores_auth_metadata_only() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(
        workspace.envs_dir().join("prod.conf"),
        "GIT_TOKEN=never-persist-this-plaintext-token\n",
    )
    .unwrap();
    let mut deploy = DeployPolicy::default();
    deploy.sources.allow_private_https_batteries = true;
    let app = router_with_deploy(
        test_credential::authenticator(&["batteries:write", "batteries:read", "credentials:use"]),
        workspace.clone_for_executor(),
        ApiPolicy::with_secret_refs(["secret://prod/GIT_TOKEN"]),
        deploy,
    );
    let add = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"private","git_url":"https://example.invalid/private.git","requested_ref":"main","token_ref":"secret://prod/GIT_TOKEN"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(add.status(), StatusCode::OK);
    let add_body = response_json(add).await;
    assert_eq!(add_body["data"]["auth"]["method"], "https_token_ref");
    assert_eq!(
        add_body["data"]["auth"]["token_ref"],
        "secret://prod/GIT_TOKEN"
    );
    let list = app.oneshot(authed_request("/v1/batteries")).await.unwrap();
    let body = response_json(list).await;
    let auth = &body["data"][0]["auth"];
    assert_eq!(auth["method"], "https_token_ref");
    assert_eq!(auth["token_ref"], "secret://prod/GIT_TOKEN");
    let registry =
        std::fs::read_to_string(battery_ops::BatteryPaths::for_workspace(&workspace).registry_path)
            .unwrap();
    assert!(!registry.contains("never-persist-this-plaintext-token"));
    assert!(registry.contains("secret://prod/GIT_TOKEN"));
}

#[tokio::test]
async fn battery_add_token_ref_denied_without_credentials_use() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let mut deploy = DeployPolicy::default();
    deploy.sources.allow_private_https_batteries = true;
    let app = router_with_deploy(
        test_credential::authenticator(&["batteries:write"]),
        workspace,
        ApiPolicy::default(),
        deploy,
    );
    let add = app
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"private","git_url":"https://example.invalid/private.git","token_ref":"secret://prod/token"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(add.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn private_https_sync_denied_without_credentials_use() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(
        workspace.envs_dir().join("creds.conf"),
        "git_token=sync-secret-value\n",
    )
    .unwrap();
    battery_ops::add_battery(
        &workspace,
        battery_ops::AddBatteryRequest {
            name: "private".into(),
            git_url: "https://example.invalid/private.git".into(),
            requested_ref: "main".into(),
            token_ref: Some("secret://creds/git_token".into()),
        },
    )
    .unwrap();
    let mut deploy = DeployPolicy::default();
    deploy.sources.allow_private_https_batteries = true;
    let app = router_with_deploy(
        test_credential::authenticator(&["batteries:write"]),
        workspace,
        ApiPolicy::default(),
        deploy,
    );
    let response = app
        .oneshot(authed_json_request("/v1/batteries/private/sync", r#"{}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = response_json(response).await;
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("credentials:use")
    );
    assert!(!body.to_string().contains("sync-secret-value"));
}

#[tokio::test]
async fn battery_endpoints_require_auth() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/batteries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn battery_add_and_list_use_operations() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let app = router(workspace);
    let add = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"azure","git_url":"https://example.invalid/azure.git","requested_ref":"stable"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(add.status(), StatusCode::OK);
    let add_body = response_json(add).await;
    assert_eq!(add_body["data"]["name"], "azure");
    assert_eq!(add_body["data"]["requested_ref"], "stable");

    let list = app.oneshot(authed_request("/v1/batteries")).await.unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = response_json(list).await;
    assert_eq!(list_body["data"][0]["name"], "azure");
}

#[tokio::test]
async fn battery_add_rejects_plaintext_http_git_url() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"plain","git_url":"http://example.invalid/plain.git"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn battery_add_rejects_local_git_url() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_json_request(
            "/v1/batteries",
            r#"{"name":"local","git_url":"/tmp/local-battery.git"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn battery_sync_invalid_manifest_maps_to_400() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    register_invalid_https_battery_cache(&workspace, "bad");

    let response = router(workspace)
        .oneshot(authed_request("/v1/batteries/bad"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "manifest_invalid");
}

#[tokio::test]
async fn battery_http_operations_reject_existing_non_https_sources() {
    let repo = invalid_manifest_repo();
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    battery_ops::add_battery(
        &workspace,
        battery_ops::AddBatteryRequest {
            name: "local".into(),
            git_url: repo.path().display().to_string(),
            requested_ref: "main".into(),
            token_ref: None,
        },
    )
    .unwrap();

    let app = router(workspace);
    for request in [
        authed_json_request("/v1/batteries/local/sync", r#"{}"#),
        authed_request("/v1/batteries/local"),
        authed_request("/v1/batteries/local/scripts"),
        authed_json_request("/v1/batteries/local/scripts/list/install", r#"{}"#),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }
}

#[tokio::test]
async fn battery_missing_and_unsynced_errors_are_stable() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    battery_ops::add_battery(
        &workspace,
        battery_ops::AddBatteryRequest {
            name: "azure".into(),
            git_url: "https://example.invalid/azure.git".into(),
            requested_ref: "main".into(),
            token_ref: None,
        },
    )
    .unwrap();

    let app = router(workspace);
    let missing = app
        .clone()
        .oneshot(authed_request("/v1/batteries/missing"))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing_body = response_json(missing).await;
    assert_eq!(missing_body["error"]["code"], "not_found");

    let unsynced = app
        .clone()
        .oneshot(authed_request("/v1/batteries/azure/scripts"))
        .await
        .unwrap();
    assert_eq!(unsynced.status(), StatusCode::CONFLICT);
    let unsynced_body = response_json(unsynced).await;
    assert_eq!(unsynced_body["error"]["code"], "not_synced");

    let install = app
        .oneshot(authed_json_request(
            "/v1/batteries/azure/scripts/list/install",
            r#"{}"#,
        ))
        .await
        .unwrap();
    assert_eq!(install.status(), StatusCode::CONFLICT);
    let install_body = response_json(install).await;
    #[cfg(unix)]
    assert_eq!(install_body["error"]["code"], "not_synced");
    #[cfg(not(unix))]
    assert_eq!(install_body["error"]["code"], "conflict");
}

#[tokio::test]
async fn battery_sync_missing_maps_to_404() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_json_request("/v1/batteries/missing/sync", r#"{}"#))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn battery_remove_supports_cache_flag() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    battery_ops::add_battery(
        &workspace,
        battery_ops::AddBatteryRequest {
            name: "keep".into(),
            git_url: "https://example.invalid/keep.git".into(),
            requested_ref: "main".into(),
            token_ref: None,
        },
    )
    .unwrap();
    battery_ops::add_battery(
        &workspace,
        battery_ops::AddBatteryRequest {
            name: "drop".into(),
            git_url: "https://example.invalid/drop.git".into(),
            requested_ref: "main".into(),
            token_ref: None,
        },
    )
    .unwrap();
    let paths = battery_ops::BatteryPaths::for_workspace(&workspace);
    std::fs::create_dir_all(paths.cache_path_for("drop")).unwrap();

    let app = router(workspace);
    let keep = app
        .clone()
        .oneshot(authed_delete_request("/v1/batteries/keep"))
        .await
        .unwrap();
    assert_eq!(keep.status(), StatusCode::OK);
    let keep_body = response_json(keep).await;
    assert_eq!(keep_body["data"]["cache_removed"], false);

    let drop = app
        .oneshot(authed_delete_request(
            "/v1/batteries/drop?remove_cache=true",
        ))
        .await
        .unwrap();
    assert_eq!(drop.status(), StatusCode::OK);
    let drop_body = response_json(drop).await;
    assert_eq!(drop_body["data"]["cache_removed"], true);
}
