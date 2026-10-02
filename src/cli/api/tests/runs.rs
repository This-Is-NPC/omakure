use super::*;
use crate::runs::{self, EnqueueOptions, RunCompletion};

#[tokio::test]
async fn runs_and_queue_stats_endpoints_return_operation_data() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let conn = runs::open(&workspace).unwrap();
    runs::enqueue(
        &conn,
        workspace
            .scripts_root()
            .join("job.sh")
            .to_string_lossy()
            .as_ref(),
        &[],
        EnqueueOptions {
            run_id: Some("rid-http".into()),
            actor: "agent".into(),
            reason: None,
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
            script_content_hash: None,
        },
    )
    .unwrap();

    let app = router(workspace);
    let list = app
        .clone()
        .oneshot(authed_request("/v1/runs?state_set=all"))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = response_json(list).await;
    assert_eq!(list_body["data"][0]["run_id"], "rid-http");

    let show = app
        .clone()
        .oneshot(authed_request("/v1/runs/rid-http"))
        .await
        .unwrap();
    assert_eq!(show.status(), StatusCode::OK);
    let show_body = response_json(show).await;
    assert_eq!(show_body["data"]["actor"], "agent");

    let traces = app
        .clone()
        .oneshot(authed_request("/v1/runs/rid-http/traces"))
        .await
        .unwrap();
    assert_eq!(traces.status(), StatusCode::OK);

    let stats = app
        .oneshot(authed_request("/v1/queue/stats"))
        .await
        .unwrap();
    assert_eq!(stats.status(), StatusCode::OK);
    let stats_body = response_json(stats).await;
    assert_eq!(stats_body["data"]["total"], 1);
}

#[tokio::test]
async fn runs_endpoint_maps_invalid_query_to_400() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_request("/v1/runs?state=bad"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn enqueue_run_requires_auth() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/runs")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"script":"job.sh"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn enqueue_run_returns_queued_run() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");

    let response = router(workspace)
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"job","args":["--x"],"run_id":"rid-post","actor":"agent","reason":"api","priority":7,"timeout_ms":1000}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["data"]["run_id"], "rid-post");
    assert_eq!(body["data"]["state"], "queued");
    assert_eq!(body["data"]["actor"], "agent");
    assert_eq!(body["data"]["priority"], 7);
}

#[tokio::test]
async fn enqueue_run_endpoint_redacts_secret_args_in_response_and_storage() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(
        workspace.scripts_root().join("secret.sh"),
        r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
    )
    .unwrap();
    std::fs::write(
        workspace.envs_dir().join("prod.conf"),
        "token=http_secret_value\n",
    )
    .unwrap();

    let response = router(workspace.clone_for_executor())
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret.sh","run_id":"rid-http-secret","args":["--token","secret://prod/token"]}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let rendered = body.to_string();
    assert!(rendered.contains("secret://prod/token"));
    assert!(!rendered.contains("http_secret_value"));

    let conn = runs::open(&workspace).unwrap();
    let row = runs::get_run(&conn, "rid-http-secret").unwrap().unwrap();
    assert!(row.args_json.contains("secret://prod/token"));
    assert!(!row.args_json.contains("http_secret_value"));
}

#[tokio::test]
async fn enqueue_run_rejects_plaintext_secret_arg_values() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(
        workspace.scripts_root().join("secret.sh"),
        r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
    )
    .unwrap();

    let response = router(workspace)
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret.sh","run_id":"rid-http-plain-secret","args":["--token","http_secret_value"]}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_input");
    assert!(!body.to_string().contains("http_secret_value"));
}

#[tokio::test]
async fn enqueue_run_accepts_env_and_rejects_non_reconstructable_secret_fields() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(
        workspace.scripts_root().join("secret.sh"),
        r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"},{"Name":"MODE","Type":"string","Arg":"--mode"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
    )
    .unwrap();
    std::fs::write(
        workspace.envs_dir().join("prod.conf"),
        "TOKEN=env_secret_value\n",
    )
    .unwrap();

    let env_response = router(workspace.clone_for_executor())
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret.sh","run_id":"rid-http-env-secret","args":["--mode","fast"],"env":"prod"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(env_response.status(), StatusCode::OK);
    let env_body = response_json(env_response).await;
    assert!(env_body.to_string().contains("<redacted>"));
    assert!(!env_body.to_string().contains("env_secret_value"));

    let conn = runs::open(&workspace).unwrap();
    let env_row = runs::get_run(&conn, "rid-http-env-secret")
        .unwrap()
        .unwrap();
    assert_eq!(
        runs::get_run_env(&conn, "rid-http-env-secret")
            .unwrap()
            .as_deref(),
        Some("prod")
    );
    assert!(env_row.args_json.contains("<redacted>"));
    assert!(!env_row.args_json.contains("env_secret_value"));
    drop(conn);

    let direct_response = router(workspace.clone_for_executor())
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret.sh","run_id":"rid-http-direct-secret","secret_fields":{"TOKEN":"direct_secret_value"}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(direct_response.status(), StatusCode::BAD_REQUEST);
    let direct_body = response_json(direct_response).await;
    assert_eq!(direct_body["error"]["code"], "invalid_input");
    assert!(!direct_body.to_string().contains("direct_secret_value"));
}

#[tokio::test]
async fn enqueue_run_enforces_secret_provider_acl() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(
        workspace.scripts_root().join("secret.sh"),
        r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
    )
    .unwrap();
    std::fs::write(
        workspace.envs_dir().join("prod.conf"),
        "token=provider_secret\n",
    )
    .unwrap();
    let app = router_with_policy(
        test_credential::authenticator(&["runs:write", "secrets:use"]),
        workspace.clone_for_executor(),
        ApiPolicy::with_secret_refs(["secret://prod/other"]),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );

    let denied = app
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret.sh","run_id":"rid-denied-ref","args":["--token","secret://prod/token"]}"#,
        ))
        .await
        .unwrap();

    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let body = response_json(denied).await;
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(!body.to_string().contains("provider_secret"));
}

#[tokio::test]
async fn enqueue_run_env_and_secret_fields_require_policy_capabilities() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    std::fs::write(
        workspace.envs_dir().join("prod.conf"),
        "TOKEN=env_secret_value\n",
    )
    .unwrap();
    let app = router_with_policy(
        test_credential::authenticator(&["runs:write"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );

    let plain = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"job.sh","run_id":"rid-plain"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);

    for body in [
        r#"{"script":"job.sh","run_id":"rid-env","env":"prod"}"#,
        r#"{"script":"job.sh","run_id":"rid-secret","secret_fields":{"TOKEN":"direct_secret_value"}}"#,
        r#"{"script":"job.sh","run_id":"rid-secret-ref","args":["--token","secret://prod/token"]}"#,
    ] {
        let response = app
            .clone()
            .oneshot(authed_json_request("/v1/runs", body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
    }
}

#[tokio::test]
async fn enqueue_run_secret_fields_policy_denies_all_provider_entry_points() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_secret_script(workspace.scripts_root(), "secret.sh", None);
    write_secret_script(
        workspace.scripts_root(),
        "secret-default.sh",
        Some("secret://prod/token"),
    );
    let mut deploy = DeployPolicy::default();
    deploy.runs.allow_secret_fields = false;
    let app = router_with_policy(
        test_credential::authenticator(&["*"]),
        workspace.clone_for_executor(),
        ApiPolicy::default(),
        deploy,
        None,
        BODY_LIMIT_BYTES,
    );

    for (run_id, body) in [
        (
            "rid-policy-args",
            r#"{"script":"secret.sh","run_id":"rid-policy-args","args":["--token","secret://prod/token"]}"#,
        ),
        (
            "rid-policy-fields",
            r#"{"script":"secret.sh","run_id":"rid-policy-fields","secret_fields":{"TOKEN":"secret://prod/token"}}"#,
        ),
        (
            "rid-policy-default",
            r#"{"script":"secret-default.sh","run_id":"rid-policy-default"}"#,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(authed_json_request("/v1/runs", body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{run_id}");
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert_eq!(
            body["error"]["message"],
            "policy runs.allow_secret_fields=false"
        );
    }

    let conn = runs::open(&workspace).unwrap();
    for run_id in ["rid-policy-args", "rid-policy-fields", "rid-policy-default"] {
        assert!(runs::get_run(&conn, run_id).unwrap().is_none());
    }
}

#[tokio::test]
async fn enqueue_run_implicit_secret_default_requires_secret_capability() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_secret_script(
        workspace.scripts_root(),
        "secret-default.sh",
        Some("schema_secret_value"),
    );
    let app = router_with_policy(
        test_credential::authenticator(&["runs:write"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );

    let response = app
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret-default.sh","run_id":"rid-default"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(!body.to_string().contains("schema_secret_value"));
}

#[tokio::test]
async fn enqueue_run_implicit_active_env_secret_requires_env_capability() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_secret_script(workspace.scripts_root(), "secret-env.sh", None);
    std::fs::write(
        workspace.envs_dir().join("prod.conf"),
        "token=active_secret\n",
    )
    .unwrap();
    std::fs::write(workspace.envs_active_path(), "prod.conf\n").unwrap();
    let app = router_with_policy(
        test_credential::authenticator(&["runs:write"]),
        workspace,
        ApiPolicy::default(),
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    );

    let response = app
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"secret-env.sh","run_id":"rid-env-implicit"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(!body.to_string().contains("active_secret"));
}

#[tokio::test]
async fn mutating_run_routes_require_run_write_capability() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let conn = runs::open(&workspace).unwrap();
    runs::enqueue(
        &conn,
        "job.sh",
        &[],
        EnqueueOptions {
            run_id: Some("rid-cap".into()),
            ..Default::default()
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
        authed_json_request("/v1/runs/rid-cap/cancel", r#"{}"#),
        authed_json_request("/v1/runs/rid-cap/dead-letter", r#"{}"#),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
    }
}

#[tokio::test]
async fn enqueue_run_maps_invalid_script_to_404() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_json_request(
            "/v1/runs",
            r#"{"script":"missing.sh"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn enqueue_run_rejects_outside_workspace_script() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(outside.path(), "outside.sh");

    let outside_script = outside.path().join("outside.sh").display().to_string();
    let request_body =
        serde_json::to_string(&serde_json::json!({ "script": outside_script })).unwrap();
    let response = router(workspace)
        .oneshot(authed_json_request("/v1/runs", &request_body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "unsafe_path");
}

#[tokio::test]
async fn cancel_run_success_and_missing_run() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let conn = runs::open(&workspace).unwrap();
    runs::enqueue(
        &conn,
        workspace
            .scripts_root()
            .join("job.sh")
            .to_string_lossy()
            .as_ref(),
        &[],
        EnqueueOptions {
            run_id: Some("rid-cancel".into()),
            actor: "agent".into(),
            reason: None,
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
            script_content_hash: None,
        },
    )
    .unwrap();

    let app = router(workspace);
    let cancelled = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/runs/rid-cancel/cancel",
            r#"{"reason":"stop"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled_body = response_json(cancelled).await;
    assert_eq!(cancelled_body["data"]["state"], "cancelled");

    let missing = app
        .oneshot(authed_json_request("/v1/runs/missing/cancel", r#"{}"#))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cancel_run_maps_invalid_transition_to_conflict() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let conn = runs::open(&workspace).unwrap();
    let row = runs::start_inline(
        &conn,
        workspace
            .scripts_root()
            .join("job.sh")
            .to_string_lossy()
            .as_ref(),
        &[],
        "worker:test",
        EnqueueOptions {
            run_id: Some("rid-done".into()),
            actor: "agent".into(),
            reason: None,
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
            script_content_hash: None,
        },
    )
    .unwrap();
    runs::complete(
        &conn,
        &row.run_id,
        RunCompletion {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            success: true,
            error: None,
        },
    )
    .unwrap();

    let response = router(workspace)
        .oneshot(authed_json_request("/v1/runs/rid-done/cancel", r#"{}"#))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "conflict");
}

#[tokio::test]
async fn dead_letter_run_success_and_invalid_transition() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    write_script(workspace.scripts_root(), "job.sh");
    let conn = runs::open(&workspace).unwrap();
    let failed = runs::start_inline(
        &conn,
        workspace
            .scripts_root()
            .join("job.sh")
            .to_string_lossy()
            .as_ref(),
        &[],
        "worker:test",
        EnqueueOptions {
            run_id: Some("rid-failed".into()),
            actor: "agent".into(),
            reason: None,
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
            script_content_hash: None,
        },
    )
    .unwrap();
    runs::fail(
        &conn,
        &failed.run_id,
        RunCompletion {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(1),
            success: false,
            error: Some("boom".into()),
        },
    )
    .unwrap();
    runs::enqueue(
        &conn,
        workspace
            .scripts_root()
            .join("job.sh")
            .to_string_lossy()
            .as_ref(),
        &[],
        EnqueueOptions {
            run_id: Some("rid-queued".into()),
            actor: "agent".into(),
            reason: None,
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
            script_content_hash: None,
        },
    )
    .unwrap();

    let app = router(workspace);
    let promoted = app
        .clone()
        .oneshot(authed_json_request(
            "/v1/runs/rid-failed/dead-letter",
            r#"{"reason":"triaged"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(promoted.status(), StatusCode::OK);
    let promoted_body = response_json(promoted).await;
    assert_eq!(promoted_body["data"]["state"], "dead_letter");

    let invalid = app
        .oneshot(authed_json_request(
            "/v1/runs/rid-queued/dead-letter",
            r#"{}"#,
        ))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::CONFLICT);
    let invalid_body = response_json(invalid).await;
    assert_eq!(invalid_body["error"]["code"], "conflict");
}
