mod support;

use std::time::Duration;

#[test]
fn support_harness_provides_workspace_script_json_and_secret_assertions() {
    let workspace = support::TestWorkspace::new("harness_self_test");
    let script = workspace.write_schema_script("hello.sh", "hello", "echo fixture-ok");
    support::set_executable(&script);

    let output = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .arg("--json")
            .arg("run")
            .arg("hello.sh"),
        Duration::from_secs(10),
    );

    assert!(
        output.status.success(),
        "expected run success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = support::json_envelope(&output.stdout);
    assert_eq!(envelope["ok"], true);

    support::assert_redacted("token=[REDACTED]", "super-secret-token");
    support::assert_no_secret_leak(&output.stdout, b"super-secret-token");
}

#[test]
fn support_harness_provides_http_port_readiness_and_teardown() {
    let workspace = support::TestWorkspace::new("harness_http_self_test");
    let token = "harness-token-0123456789abcdef012345";
    let server = support::HttpServer::start(workspace.path(), token, Duration::from_secs(10));

    let body = support::http_get_with_timeout(
        &server.url("/v1/health"),
        Some(token),
        Duration::from_secs(5),
    );
    let envelope = support::json_envelope(body.as_bytes());
    assert_eq!(envelope["ok"], true);
}
