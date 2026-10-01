use super::*;

#[tokio::test(flavor = "current_thread")]
async fn script_routes_use_the_shared_blocking_gate() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    gate.close();
    let app = super::super::router::router_with_blocking_gate(workspace, gate);

    for path in ["/v1/scripts/job.sh/content", "/v1/search?q=job"] {
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
async fn scripts_and_schema_endpoints_return_operation_data() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");

    let app = router(workspace);
    let list = app
        .clone()
        .oneshot(authed_request("/v1/scripts?tag=ops"))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = response_json(list).await;
    assert_eq!(list_body["data"][0]["relative_path"], "job.sh");

    let show = app
        .clone()
        .oneshot(authed_request("/v1/scripts/job.sh"))
        .await
        .unwrap();
    assert_eq!(show.status(), StatusCode::OK);
    let show_body = response_json(show).await;
    assert_eq!(show_body["data"]["schema"]["name"], "job.sh");

    let schema = app
        .oneshot(authed_request("/v1/scripts/job.sh/schema"))
        .await
        .unwrap();
    assert_eq!(schema.status(), StatusCode::OK);
    let schema_body = response_json(schema).await;
    assert_eq!(schema_body["data"]["tags"][0], "ops");
}

#[tokio::test]
async fn search_endpoint_returns_operation_data() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "deploy.sh");
    let response = router(workspace)
        .oneshot(authed_request("/v1/search?q=deploy&tag=ops"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"][0]["relative_path"], "deploy.sh");
}

#[tokio::test]
async fn search_endpoint_refreshes_changes() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let root = workspace.scripts_root().to_path_buf();
    let app = router(workspace);
    let empty = app
        .clone()
        .oneshot(authed_request("/v1/search?q=job"))
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(response_json(empty).await["data"], serde_json::json!([]));
    std::fs::create_dir(root.join("tools")).unwrap();
    write_script(&root.join("tools"), "job.sh");
    let added = app
        .clone()
        .oneshot(authed_request("/v1/search?q=job"))
        .await
        .unwrap();
    assert_eq!(added.status(), StatusCode::OK);
    assert_eq!(
        response_json(added).await["data"][0]["relative_path"],
        "tools/job.sh"
    );
    let path = root.join("tools/job.sh");
    let content = std::fs::read_to_string(&path)
        .unwrap()
        .replace("job.sh", "updated.sh");
    std::fs::write(&path, content).unwrap();
    let changed = app
        .clone()
        .oneshot(authed_request("/v1/search?q=updated"))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(
        response_json(changed).await["data"][0]["name"],
        "updated.sh"
    );

    std::fs::remove_file(path).unwrap();
    let removed = app
        .oneshot(authed_request("/v1/search?q=updated"))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);
    assert_eq!(response_json(removed).await["data"], serde_json::json!([]));
}

#[tokio::test]
async fn search_endpoint_requires_query() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_request("/v1/search"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn search_endpoint_rejects_empty_and_oversized_query() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let app = router(workspace);

    let empty = app
        .clone()
        .oneshot(authed_request("/v1/search?q="))
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    let too_long = "x".repeat(MAX_SEARCH_QUERY_LEN + 1);
    let long = app
        .oneshot(authed_request(&format!("/v1/search?q={too_long}")))
        .await
        .unwrap();
    assert_eq!(long.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn search_endpoint_rejects_excessive_tags() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let app = router(workspace);

    let too_many_tags = (0..=MAX_SEARCH_TAGS)
        .map(|idx| format!("tag=t{idx}"))
        .collect::<Vec<_>>()
        .join("&");
    let too_many = app
        .clone()
        .oneshot(authed_request(&format!(
            "/v1/search?q=deploy&{too_many_tags}"
        )))
        .await
        .unwrap();
    assert_eq!(too_many.status(), StatusCode::BAD_REQUEST);

    let too_long_tag = "x".repeat(MAX_SEARCH_TAG_LEN + 1);
    let long = app
        .oneshot(authed_request(&format!(
            "/v1/search?q=deploy&tag={too_long_tag}"
        )))
        .await
        .unwrap();
    assert_eq!(long.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn script_routes_support_nested_paths() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "tools/job.sh");
    std::fs::write(
        workspace.scripts_root().join("tools/secret.sh"),
        r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Default":"schema_secret_default","Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
    )
    .unwrap();

    let app = router(workspace);
    let show = app
        .clone()
        .oneshot(authed_request("/v1/scripts/tools/job.sh"))
        .await
        .unwrap();
    assert_eq!(show.status(), StatusCode::OK);
    let show_body = response_json(show).await;
    assert_eq!(show_body["data"]["relative_path"], "tools/job.sh");

    let schema = app
        .clone()
        .oneshot(authed_request("/v1/scripts/tools/job.sh/schema"))
        .await
        .unwrap();
    assert_eq!(schema.status(), StatusCode::OK);
    let schema_body = response_json(schema).await;
    assert_eq!(schema_body["data"]["name"], "tools/job.sh");

    let secret_schema = app
        .oneshot(authed_request("/v1/scripts/tools/secret.sh/schema"))
        .await
        .unwrap();
    assert_eq!(secret_schema.status(), StatusCode::OK);
    let secret_body = response_json(secret_schema).await;
    assert!(!secret_body.to_string().contains("schema_secret_default"));
}

#[tokio::test]
async fn tree_and_content_endpoints_return_safe_browsing_data() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "tools/job.sh");

    let app = router(workspace);
    let tree = app
        .clone()
        .oneshot(authed_request("/v1/tree"))
        .await
        .unwrap();
    assert_eq!(tree.status(), StatusCode::OK);
    let tree_body = response_json(tree).await;
    assert_eq!(tree_body["data"][0]["kind"], "directory");
    assert_eq!(tree_body["data"][0]["relative_path"], "tools");

    let nested = app
        .clone()
        .oneshot(authed_request("/v1/tree/tools"))
        .await
        .unwrap();
    assert_eq!(nested.status(), StatusCode::OK);
    let nested_body = response_json(nested).await;
    assert_eq!(nested_body["data"][0]["relative_path"], "tools/job.sh");

    let content = app
        .oneshot(authed_request("/v1/scripts/tools/job.sh/content"))
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);
    let content_body = response_json(content).await;
    assert_eq!(content_body["data"]["relative_path"], "tools/job.sh");
    assert!(content_body["data"]["content"]
        .as_str()
        .unwrap()
        .contains("echo ok"));
}

#[tokio::test]
async fn content_endpoint_rejects_path_traversal() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_request("/v1/scripts/../secret.sh/content"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "unsafe_path");
}

#[tokio::test]
async fn content_endpoint_rejects_absolute_encoded_path() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_request("/v1/scripts/%2Ftmp%2Fsecret.sh/content"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "unsafe_path");
}

#[tokio::test]
async fn tree_and_content_endpoints_reject_metadata_paths() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let app = router(workspace);

    for uri in [
        "/v1/tree/.omakure",
        "/v1/tree/.history",
        "/v1/tree/.git",
        "/v1/scripts/.omakure/secret.sh/content",
        "/v1/scripts/.history/secret.sh/content",
        "/v1/scripts/.git/secret.sh/content",
    ] {
        let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsafe_path");
    }
}

#[tokio::test]
async fn content_endpoint_error_hides_local_paths() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let root = workspace.root().display().to_string();

    let response = router(workspace)
        .oneshot(authed_request("/v1/scripts/../secret.sh/content"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert!(!body.to_string().contains(&root));
}

#[tokio::test]
async fn content_endpoint_maps_unsupported_script_to_415() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(workspace.scripts_root().join("note.txt"), "hello\n").unwrap();

    let response = router(workspace)
        .oneshot(authed_request("/v1/scripts/note.txt/content"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "unsupported_script");
}

#[tokio::test]
async fn scripts_query_percent_decodes_values() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");

    let response = router(workspace)
        .oneshot(authed_request("/v1/scripts?tag=ops%20team"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn scripts_endpoint_maps_missing_script_to_404() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_request("/v1/scripts/missing.sh"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "not_found");
}
