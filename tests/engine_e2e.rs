mod support;

use serde_json::json;
use std::path::Path;
use std::time::Duration;

const API_TOKEN: &str = "engine-e2e-token-with-enough-entropy-00001";

fn write_echo_script(root: &Path, name: &str) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create script parent");
    }
    let script = r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {
#   "Name": "echo-job",
#   "Description": "engine e2e fixture",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END
echo engine-ok
"#;
    std::fs::write(&path, script).expect("write echo script");
    support::set_executable(&path);
}

#[test]
fn engine_workers_zero_no_scheduler_serves_http_like_api() {
    let workspace = support::TestWorkspace::new("engine_api_only");
    write_echo_script(workspace.path(), "echo.sh");
    let server = support::HttpServer::start_engine(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--capability",
            "scripts:read",
            "--capability",
            "runs:read",
            "--capability",
            "runs:write",
        ],
        &[],
        Duration::from_secs(15),
    );

    let health = server.get_unauthenticated("/v1/health");
    assert_eq!(health.status, 200, "body: {}", health.safe_body());

    let ready = server.get_unauthenticated("/v1/ready");
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());
    assert_eq!(ready.json()["data"]["status"], "ready");
    let ready_body = ready.body.clone();
    assert!(!ready_body.contains(API_TOKEN));
    assert!(!ready_body.contains(workspace.path().to_string_lossy().as_ref()));

    let scripts = server.get("/v1/scripts");
    assert_eq!(scripts.status, 200, "body: {}", scripts.safe_body());
    assert_eq!(scripts.json()["ok"], true);

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "echo.sh",
            "run_id": "engine-api-only-run"
        }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());
    assert_eq!(enqueue.json()["data"]["state"], "queued");

    // No in-process worker: run stays queued.
    let show = server.get("/v1/runs/engine-api-only-run");
    assert_eq!(show.status, 200, "body: {}", show.safe_body());
    assert_eq!(show.json()["data"]["state"], "queued");
}

#[test]
fn engine_workers_complete_enqueued_run_in_process() {
    let workspace = support::TestWorkspace::new("engine_workers");
    write_echo_script(workspace.path(), "echo.sh");
    let server = support::HttpServer::start_engine(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "runs:read",
            "--capability",
            "runs:write",
        ],
        &[],
        Duration::from_secs(15),
    );

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "echo.sh",
            "run_id": "engine-worker-run"
        }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let show = server.get("/v1/runs/engine-worker-run");
        assert_eq!(show.status, 200, "body: {}", show.safe_body());
        let show_json = show.json();
        let state = show_json["data"]["state"].as_str().unwrap_or("");
        if state == "completed" {
            assert!(
                show_json["data"]["stdout"]
                    .as_str()
                    .unwrap_or("")
                    .contains("engine-ok"),
                "stdout: {}",
                show.safe_body()
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run did not complete in time; last={}",
            show.safe_body()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn engine_ready_unauthenticated_and_minimal() {
    let workspace = support::TestWorkspace::new("engine_ready");
    let server = support::HttpServer::start_engine(
        workspace.path(),
        API_TOKEN,
        &["--workers", "0", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );

    let ready = server.get_unauthenticated("/v1/ready");
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());
    let json = ready.json();
    assert_eq!(json["ok"], true);
    assert_eq!(json["data"]["status"], "ready");
    let keys: Vec<_> = json["data"]
        .as_object()
        .expect("data object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, vec!["status".to_string()]);
}

#[test]
fn engine_ready_with_readiness_requires_flags_when_loops_alive() {
    let workspace = support::TestWorkspace::new("engine_ready_flags");
    let server = support::HttpServer::start_engine(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--scheduler",
            "--readiness-requires-worker",
            "--readiness-requires-scheduler",
        ],
        &[],
        Duration::from_secs(15),
    );

    let ready = server.get_unauthenticated("/v1/ready");
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());
    assert_eq!(ready.json()["data"]["status"], "ready");
}

#[cfg(unix)]
#[test]
fn engine_sigterm_stops_cleanly() {
    let workspace = support::TestWorkspace::new("engine_sigterm");
    let mut server = support::HttpServer::start_engine(
        workspace.path(),
        API_TOKEN,
        &["--workers", "1", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );

    let pid = server.child_id();
    let ready = server.get_unauthenticated("/v1/ready");
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());

    // SAFETY: libc::kill is the standard way to deliver SIGTERM to a child.
    let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill(SIGTERM) failed");

    let status = server.wait_exit(Duration::from_secs(15));
    assert!(
        status.success() || status.code() == Some(0) || status.code().is_none(),
        "engine exit status: {status:?}"
    );
}
