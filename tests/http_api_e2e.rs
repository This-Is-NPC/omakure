mod support;

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

const API_TOKEN: &str = "http-api-e2e-token-with-enough-entropy-0001";
const SECRET_DEFAULT: &str = "http-schema-secret-default-plain-value";
const QUEUE_SECRET: &str = "http-queue-secret-provider-plain-value";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteCoverage {
    Covered(&'static str),
    #[allow(dead_code)]
    Excluded(&'static str),
}

/// Inventory of every `(method, route)` declared by `src/cli/api.rs` router.
/// Coverage notes stay here; the route list itself is parsed from
/// `HTTP_ROUTE_INVENTORY` markers in `src/cli/api.rs`.
const HTTP_ROUTE_COVERAGE_NOTES: &[((&str, &str), RouteCoverage)] = &[
    (
        ("GET", "/v1/health"),
        RouteCoverage::Covered("health_and_config_family_routes"),
    ),
    (
        ("GET", "/v1/config"),
        RouteCoverage::Covered("health_and_config_family_routes"),
    ),
    (
        ("GET", "/v1/doctor"),
        RouteCoverage::Covered("health_and_config_family_routes"),
    ),
    (
        ("GET", "/v1/workspace"),
        RouteCoverage::Covered("health_and_config_family_routes"),
    ),
    (
        ("GET", "/v1/search"),
        RouteCoverage::Covered("scripts_search_tree_family_routes"),
    ),
    (
        ("GET", "/v1/tree"),
        RouteCoverage::Covered("scripts_search_tree_family_routes"),
    ),
    (
        ("GET", "/v1/tree/*path"),
        RouteCoverage::Covered("scripts_search_tree_family_routes"),
    ),
    (
        ("GET", "/v1/scripts"),
        RouteCoverage::Covered("scripts_search_tree_family_routes"),
    ),
    (
        ("GET", "/v1/scripts/*script_id"),
        RouteCoverage::Covered("scripts_search_tree_family_routes (+ /schema /content)"),
    ),
    (
        ("GET", "/v1/envs"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("POST", "/v1/envs"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("DELETE", "/v1/envs/active"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("GET", "/v1/envs/:name"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("PUT", "/v1/envs/:name"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("PATCH", "/v1/envs/:name"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("DELETE", "/v1/envs/:name"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("POST", "/v1/envs/:name/activate"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("PUT", "/v1/envs/:name/params/:key"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("DELETE", "/v1/envs/:name/params/:key"),
        RouteCoverage::Covered("envs_family_routes"),
    ),
    (
        ("GET", "/v1/runs"),
        RouteCoverage::Covered("runs_queue_family_routes + secret tests"),
    ),
    (
        ("POST", "/v1/runs"),
        RouteCoverage::Covered("runs_queue_family_routes + secret tests"),
    ),
    (
        ("GET", "/v1/runs/:run_id"),
        RouteCoverage::Covered("runs_queue_family_routes + secret tests"),
    ),
    (
        ("GET", "/v1/runs/:run_id/traces"),
        RouteCoverage::Covered("runs_queue_family_routes"),
    ),
    (
        ("POST", "/v1/runs/:run_id/cancel"),
        RouteCoverage::Covered("runs_queue_family_routes"),
    ),
    (
        ("POST", "/v1/runs/:run_id/dead-letter"),
        RouteCoverage::Covered("runs_queue_family_routes"),
    ),
    (
        ("GET", "/v1/queue/stats"),
        RouteCoverage::Covered("runs_queue_family_routes"),
    ),
    (
        ("GET", "/v1/batteries"),
        RouteCoverage::Covered("batteries_family_routes"),
    ),
    (
        ("POST", "/v1/batteries"),
        RouteCoverage::Covered("batteries_family_routes"),
    ),
    (
        ("GET", "/v1/batteries/:battery_id"),
        RouteCoverage::Covered("batteries_family_routes"),
    ),
    (
        ("DELETE", "/v1/batteries/:battery_id"),
        RouteCoverage::Covered("batteries_family_routes"),
    ),
    (
        ("GET", "/v1/batteries/:battery_id/scripts"),
        RouteCoverage::Covered("batteries_family_routes"),
    ),
    (
        (
            "POST",
            "/v1/batteries/:battery_id/scripts/:script_id/install",
        ),
        RouteCoverage::Covered("batteries_family_routes"),
    ),
    (
        ("POST", "/v1/batteries/:battery_id/sync"),
        RouteCoverage::Covered(
            "batteries_family_routes (https-only validation; no remote network)",
        ),
    ),
];

#[test]
fn http_route_inventory_maps_all_current_router_entries() {
    let from_source = parse_http_route_inventory_from_source();
    let notes: Vec<_> = HTTP_ROUTE_COVERAGE_NOTES
        .iter()
        .map(|((method, route), _)| (*method, *route))
        .collect();
    assert_eq!(
        notes, from_source,
        "HTTP_ROUTE_COVERAGE_NOTES must match HTTP_ROUTE_INVENTORY in src/cli/api.rs"
    );
    assert!(HTTP_ROUTE_COVERAGE_NOTES
        .iter()
        .all(|(_, coverage)| match coverage {
            RouteCoverage::Covered(note) | RouteCoverage::Excluded(note) => !note.trim().is_empty(),
        }));
}

fn parse_http_route_inventory_from_source() -> Vec<(&'static str, &'static str)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/api.rs");
    let source = fs::read_to_string(&path).expect("read src/cli/api.rs");
    let start = source
        .find("// OMAKURE_HTTP_ROUTE_INVENTORY_START")
        .expect("inventory start marker");
    let end = source
        .find("// OMAKURE_HTTP_ROUTE_INVENTORY_END")
        .expect("inventory end marker");
    let block = &source[start..end];
    let mut routes = Vec::new();
    let bytes = block.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Find next ("METHOD", "/path") allowing rustfmt whitespace/newlines.
        if bytes[i] != b'(' {
            i += 1;
            continue;
        }
        let rest = &block[i..];
        let Some(after_paren) = rest.strip_prefix('(') else {
            i += 1;
            continue;
        };
        let after_paren = after_paren.trim_start();
        if !after_paren.starts_with('"') {
            i += 1;
            continue;
        }
        let Some(method_end) = after_paren[1..].find('"') else {
            i += 1;
            continue;
        };
        let method = &after_paren[1..1 + method_end];
        let after_method = after_paren[1 + method_end + 1..].trim_start();
        if !after_method.starts_with(',') {
            i += 1;
            continue;
        }
        let after_comma = after_method[1..].trim_start();
        if !after_comma.starts_with('"') {
            i += 1;
            continue;
        }
        let Some(route_end) = after_comma[1..].find('"') else {
            i += 1;
            continue;
        };
        let route = &after_comma[1..1 + route_end];
        if !route.starts_with('/') {
            i += 1;
            continue;
        }
        let method: &'static str = Box::leak(method.to_string().into_boxed_str());
        let route: &'static str = Box::leak(route.to_string().into_boxed_str());
        routes.push((method, route));
        i += 1;
    }
    assert!(
        !routes.is_empty(),
        "parsed zero routes from HTTP_ROUTE_INVENTORY markers"
    );
    routes
}

#[test]
fn runs_post_is_forbidden_without_write_capability() {
    let workspace = support::TestWorkspace::new("http_deny_runs");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "scripts:read"],
        &[],
        Duration::from_secs(10),
    );

    let response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", "secret://env/OMAKURE_HTTP_QUEUE_TOKEN"]
        }),
    );

    assert_eq!(response.status, 403, "body: {}", response.safe_body());
    assert_error_code(&response.json(), "forbidden");
}

#[test]
fn script_schema_redacts_secret_defaults_over_http() {
    let workspace = support::TestWorkspace::new("http_schema_redact");
    write_secret_echo_script(workspace.path(), "secret-default.sh", Some(SECRET_DEFAULT));
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "scripts:read"],
        &[],
        Duration::from_secs(10),
    );

    let response = server.get("/v1/scripts/secret-default.sh/schema");

    assert_eq!(response.status, 200, "body: {}", response.safe_body());
    response.assert_no_secret(SECRET_DEFAULT);
    let envelope = response.json();
    let fields = envelope["data"]["fields"]
        .as_array()
        .expect("schema fields");
    let token = fields
        .iter()
        .find(|field| field["name"] == "TOKEN")
        .expect("TOKEN field");
    assert_eq!(token["default"], Value::Null);
}

#[test]
fn runs_post_rejects_plaintext_secret_fields_and_secret_args() {
    let workspace = support::TestWorkspace::new("http_reject_plaintext");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "runs:write", "--capability", "secrets:use"],
        &[],
        Duration::from_secs(10),
    );

    let secret_field_response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "secret_fields": { "TOKEN": QUEUE_SECRET }
        }),
    );
    assert_eq!(
        secret_field_response.status,
        400,
        "body: {}",
        secret_field_response.safe_body()
    );
    secret_field_response.assert_no_secret(QUEUE_SECRET);
    assert_error_contains(&secret_field_response.json(), "secret://");

    let args_response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", QUEUE_SECRET]
        }),
    );
    assert_eq!(
        args_response.status,
        400,
        "body: {}",
        args_response.safe_body()
    );
    args_response.assert_no_secret(QUEUE_SECRET);
    assert_error_contains(&args_response.json(), "secret://");
}

#[test]
fn authorized_secret_ref_enqueue_worker_and_history_do_not_leak_secret() {
    let workspace = support::TestWorkspace::new("http_secret_queue");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "runs:read",
            "--capability",
            "runs:write",
            "--capability",
            "secrets:use",
            "--secret-ref",
            "secret://env/OMAKURE_HTTP_QUEUE_TOKEN",
        ],
        &[
            ("OMAKURE_HTTP_QUEUE_TOKEN", QUEUE_SECRET),
            ("OMAKURE_EXPECTED_TOKEN", QUEUE_SECRET),
        ],
        Duration::from_secs(10),
    );

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", "secret://env/OMAKURE_HTTP_QUEUE_TOKEN"]
        }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());
    enqueue.assert_no_secret(QUEUE_SECRET);
    let run_id = enqueue.json()["data"]["run_id"]
        .as_str()
        .expect("run id")
        .to_string();

    let worker = omakure_with_env(
        workspace.path(),
        &["--json", "queue", "worker", "--once"],
        &[
            ("OMAKURE_HTTP_QUEUE_TOKEN", QUEUE_SECRET),
            ("OMAKURE_EXPECTED_TOKEN", QUEUE_SECRET),
        ],
    );
    assert_success(&worker);
    assert_no_plaintext(&worker, QUEUE_SECRET);

    let show = server.get(&format!("/v1/runs/{run_id}"));
    assert_eq!(show.status, 200, "body: {}", show.safe_body());
    show.assert_no_secret(QUEUE_SECRET);
    let show_json = show.json();
    assert_eq!(show_json["data"]["state"], "completed");
    assert_eq!(show_json["data"]["stdout"], "script-saw-redacted-ok\n");

    let history = server.get("/v1/runs");
    assert_eq!(history.status, 200, "body: {}", history.safe_body());
    history.assert_no_secret(QUEUE_SECRET);
    let serialized_history = serde_json::to_string(&history.json()).expect("serialize history");
    assert!(serialized_history.contains(&run_id));
    assert!(serialized_history.contains("script-saw-redacted-ok"));
}

#[test]
fn unauthorized_secret_provider_ref_is_forbidden_without_leaking_secret() {
    let workspace = support::TestWorkspace::new("http_secret_ref_deny");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "runs:write",
            "--capability",
            "secrets:use",
            "--secret-ref",
            "secret://env/ALLOWED_ONLY",
        ],
        &[("OMAKURE_HTTP_QUEUE_TOKEN", QUEUE_SECRET)],
        Duration::from_secs(10),
    );

    let response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", "secret://env/OMAKURE_HTTP_QUEUE_TOKEN"]
        }),
    );

    assert_eq!(response.status, 403, "body: {}", response.safe_body());
    response.assert_no_secret(QUEUE_SECRET);
    assert!(!response.body.contains("OMAKURE_HTTP_QUEUE_TOKEN"));
    assert_error_code(&response.json(), "forbidden");
}

#[test]
fn health_and_config_family_routes() {
    let workspace = support::TestWorkspace::new("http_config_family");
    let denied = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "scripts:read"],
        &[],
        Duration::from_secs(10),
    );
    for path in ["/v1/config", "/v1/doctor", "/v1/workspace"] {
        let response = denied.get(path);
        assert_eq!(
            response.status,
            403,
            "{path} should deny without config:read; body: {}",
            response.safe_body()
        );
        assert_error_code(&response.json(), "forbidden");
    }

    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "config:read"],
        &[],
        Duration::from_secs(10),
    );
    let health = server.get("/v1/health");
    assert_eq!(health.status, 200, "body: {}", health.safe_body());
    assert_eq!(health.json()["ok"], true);

    for path in ["/v1/config", "/v1/doctor", "/v1/workspace"] {
        let response = server.get(path);
        assert_eq!(
            response.status,
            200,
            "{path} body: {}",
            response.safe_body()
        );
        assert_eq!(response.json()["ok"], true);
    }
}

#[test]
fn scripts_search_tree_family_routes() {
    let workspace = support::TestWorkspace::new("http_scripts_family");
    fs::create_dir_all(workspace.path().join("tools")).expect("tools dir");
    let script = workspace.write_schema_script("tools/job.sh", "job", "echo ok");
    support::set_executable(&script);

    let denied = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "config:read"],
        &[],
        Duration::from_secs(10),
    );
    for path in [
        "/v1/scripts",
        "/v1/search?q=job",
        "/v1/tree",
        "/v1/scripts/tools/job.sh",
    ] {
        let response = denied.get(path);
        assert_eq!(
            response.status,
            403,
            "{path} should deny without scripts:read; body: {}",
            response.safe_body()
        );
    }

    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "scripts:read"],
        &[],
        Duration::from_secs(10),
    );

    let scripts = server.get("/v1/scripts");
    assert_eq!(scripts.status, 200, "body: {}", scripts.safe_body());
    assert_eq!(scripts.json()["ok"], true);

    let describe = server.get("/v1/scripts/tools/job.sh");
    assert_eq!(describe.status, 200, "body: {}", describe.safe_body());
    assert_eq!(describe.json()["data"]["relative_path"], "tools/job.sh");

    let schema = server.get("/v1/scripts/tools/job.sh/schema");
    assert_eq!(schema.status, 200, "body: {}", schema.safe_body());
    assert_eq!(schema.json()["data"]["name"], "job");

    let content = server.get("/v1/scripts/tools/job.sh/content");
    assert_eq!(content.status, 200, "body: {}", content.safe_body());
    assert!(content.json()["data"]["content"]
        .as_str()
        .unwrap_or("")
        .contains("OMAKURE_SCHEMA_START"));

    let search = server.get("/v1/search?q=job");
    assert_eq!(search.status, 200, "body: {}", search.safe_body());
    assert_eq!(search.json()["ok"], true);

    let tree = server.get("/v1/tree");
    assert_eq!(tree.status, 200, "body: {}", tree.safe_body());
    assert_eq!(tree.json()["ok"], true);

    let tree_path = server.get("/v1/tree/tools");
    assert_eq!(tree_path.status, 200, "body: {}", tree_path.safe_body());
    assert_eq!(tree_path.json()["ok"], true);
}

#[test]
fn envs_family_routes() {
    let workspace = support::TestWorkspace::new("http_envs_family");
    let denied = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "config:read"],
        &[],
        Duration::from_secs(10),
    );
    let deny_list = denied.get("/v1/envs");
    assert_eq!(deny_list.status, 403, "body: {}", deny_list.safe_body());

    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "env:read",
            "--capability",
            "env:write",
            "--capability",
            "env:activate",
        ],
        &[],
        Duration::from_secs(10),
    );

    let create = server.post_json(
        "/v1/envs",
        &json!({
            "name": "prod",
            "params": [
                {"key": "HOST", "value": "prod.example.test"},
                {"key": "API_KEY", "value": "env-secret-value"}
            ]
        }),
    );
    assert_eq!(create.status, 200, "body: {}", create.safe_body());

    let show = server.get("/v1/envs/prod");
    assert_eq!(show.status, 200, "body: {}", show.safe_body());
    show.assert_no_secret("env-secret-value");
    let show_json = show.json();
    let show_entries = show_json["data"].as_array().expect("env entries");
    let api_key = show_entries
        .iter()
        .find(|entry| entry["key"] == "API_KEY")
        .expect("API_KEY");
    assert_eq!(api_key["value"], "****");

    let put = server.put_json(
        "/v1/envs/prod",
        &json!({
            "params": [
                {"key": "HOST", "value": "replaced.example.test"},
                {"key": "TOKEN", "value": "replaced-secret"}
            ]
        }),
    );
    assert_eq!(put.status, 200, "body: {}", put.safe_body());

    let show_after_put = server.get("/v1/envs/prod");
    assert_eq!(
        show_after_put.status,
        200,
        "body: {}",
        show_after_put.safe_body()
    );
    show_after_put.assert_no_secret("replaced-secret");
    show_after_put.assert_no_secret("env-secret-value");
    let put_json = show_after_put.json();
    let put_entries = put_json["data"].as_array().expect("env entries after put");
    let token = put_entries
        .iter()
        .find(|entry| entry["key"] == "TOKEN")
        .expect("TOKEN");
    assert_eq!(token["value"], "****");

    let patch = server.patch_json(
        "/v1/envs/prod",
        &json!({
            "params": [{"key": "PORT", "value": "443"}]
        }),
    );
    assert_eq!(patch.status, 200, "body: {}", patch.safe_body());

    let set_param = server.put_json("/v1/envs/prod/params/REGION", &json!({"value": "us-east"}));
    assert_eq!(set_param.status, 200, "body: {}", set_param.safe_body());

    let activate = server.post_json("/v1/envs/prod/activate", &json!({}));
    assert_eq!(activate.status, 200, "body: {}", activate.safe_body());

    let list = server.get("/v1/envs");
    assert_eq!(list.status, 200, "body: {}", list.safe_body());
    assert_eq!(list.json()["data"][0]["active"], true);

    let delete_param = server.delete("/v1/envs/prod/params/TOKEN");
    assert_eq!(
        delete_param.status,
        200,
        "body: {}",
        delete_param.safe_body()
    );

    let deactivate = server.delete("/v1/envs/active");
    assert_eq!(deactivate.status, 200, "body: {}", deactivate.safe_body());

    let delete = server.delete("/v1/envs/prod");
    assert_eq!(delete.status, 200, "body: {}", delete.safe_body());
}

#[test]
fn runs_queue_family_routes() {
    let workspace = support::TestWorkspace::new("http_runs_family");
    let script = workspace.write_schema_script("job.sh", "job", "exit 0");
    support::set_executable(&script);
    let fail = workspace.write_schema_script("fail.sh", "fail", "exit 7");
    support::set_executable(&fail);

    let denied = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "scripts:read"],
        &[],
        Duration::from_secs(10),
    );
    let deny_stats = denied.get("/v1/queue/stats");
    assert_eq!(deny_stats.status, 403, "body: {}", deny_stats.safe_body());

    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "runs:read", "--capability", "runs:write"],
        &[],
        Duration::from_secs(10),
    );

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "job.sh",
            "run_id": "http-cancel-me",
            "priority": 3
        }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());
    assert_eq!(enqueue.json()["data"]["state"], "queued");

    let cancel = server.post_json("/v1/runs/http-cancel-me/cancel", &json!({}));
    assert_eq!(cancel.status, 200, "body: {}", cancel.safe_body());
    assert_eq!(cancel.json()["data"]["state"], "cancelled");

    let fail_enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "fail.sh",
            "run_id": "http-dead-letter"
        }),
    );
    assert_eq!(
        fail_enqueue.status,
        200,
        "body: {}",
        fail_enqueue.safe_body()
    );
    let worker = omakure(workspace.path(), &["--json", "queue", "worker", "--once"]);
    assert_success(&worker);

    let dead = server.post_json("/v1/runs/http-dead-letter/dead-letter", &json!({}));
    assert_eq!(dead.status, 200, "body: {}", dead.safe_body());
    assert_eq!(dead.json()["data"]["state"], "dead_letter");

    let show = server.get("/v1/runs/http-dead-letter");
    assert_eq!(show.status, 200, "body: {}", show.safe_body());

    let traces = server.get("/v1/runs/http-dead-letter/traces");
    assert_eq!(traces.status, 200, "body: {}", traces.safe_body());
    assert_eq!(traces.json()["ok"], true);

    let list = server.get("/v1/runs?state_set=all");
    assert_eq!(list.status, 200, "body: {}", list.safe_body());

    let stats = server.get("/v1/queue/stats");
    assert_eq!(stats.status, 200, "body: {}", stats.safe_body());
    assert!(stats.json()["data"]["counts_by_state"].is_object());
}

#[test]
fn batteries_family_routes() {
    let workspace = support::TestWorkspace::new("http_batteries_family");
    let repo = support::TestWorkspace::new("http_batteries_repo");
    support::write_local_battery_repo(repo.path(), "fixture", "HTTP battery fixture");

    let denied = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "config:read"],
        &[],
        Duration::from_secs(10),
    );
    let deny_list = denied.get("/v1/batteries");
    assert_eq!(deny_list.status, 403, "body: {}", deny_list.safe_body());

    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "batteries:read",
            "--capability",
            "batteries:write",
        ],
        &[],
        Duration::from_secs(10),
    );

    // Non-https registration is rejected at the HTTP boundary (no network).
    let reject_local = server.post_json(
        "/v1/batteries",
        &json!({
            "name": "local",
            "git_url": repo.path().to_str().expect("repo path"),
            "requested_ref": "main"
        }),
    );
    assert_eq!(
        reject_local.status,
        400,
        "body: {}",
        reject_local.safe_body()
    );
    assert_error_contains(&reject_local.json(), "https");

    // Register via CLI with a local path, then rewrite registry to https URL
    // so inspect/scripts/install/sync/delete exercise the HTTP surface without
    // contacting a remote host.
    let add = omakure(
        workspace.path(),
        &[
            "--json",
            "battery",
            "add",
            repo.path().to_str().expect("repo path"),
            "--name",
            "fixture",
        ],
    );
    assert_success(&add);
    let sync = omakure(workspace.path(), &["--json", "battery", "sync", "fixture"]);
    assert_success(&sync);
    rewrite_battery_git_url_to_https(workspace.path(), "fixture");

    let list = server.get("/v1/batteries");
    assert_eq!(list.status, 200, "body: {}", list.safe_body());
    assert_eq!(list.json()["ok"], true);

    let inspect = server.get("/v1/batteries/fixture");
    assert_eq!(inspect.status, 200, "body: {}", inspect.safe_body());
    assert_eq!(inspect.json()["data"]["summary"]["name"], "fixture");

    let scripts = server.get("/v1/batteries/fixture/scripts");
    assert_eq!(scripts.status, 200, "body: {}", scripts.safe_body());
    assert!(scripts.json()["data"]
        .as_array()
        .expect("battery scripts")
        .iter()
        .any(|entry| entry["id"] == "local.echo"));

    let install = server.post_json(
        "/v1/batteries/fixture/scripts/local.echo/install",
        &json!({ "force": true }),
    );
    assert_eq!(install.status, 200, "body: {}", install.safe_body());
    assert!(workspace.path().join("scripts/echo.sh").exists());

    // Sync route: nearest safe boundary is https-only validation without
    // contacting a remote host (local git URL is rejected).
    rewrite_battery_git_url(workspace.path(), "fixture", "file:///tmp/not-a-remote.git");
    let sync_http = server.post_json("/v1/batteries/fixture/sync", &json!({}));
    assert_eq!(
        sync_http.status,
        400,
        "sync must reject non-https sources; body: {}",
        sync_http.safe_body()
    );
    assert_eq!(sync_http.json()["ok"], false);
    assert_error_code(&sync_http.json(), "invalid_input");
    assert_error_contains(&sync_http.json(), "https");

    let remove = server.delete("/v1/batteries/fixture");
    assert_eq!(remove.status, 200, "body: {}", remove.safe_body());
}

#[test]
fn protected_routes_return_401_without_or_with_invalid_bearer() {
    let workspace = support::TestWorkspace::new("http_auth_401");
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "config:read"],
        &[],
        Duration::from_secs(10),
    );

    // Health remains reachable without a token.
    let health = server.get_unauthenticated("/v1/health");
    assert_eq!(health.status, 200, "body: {}", health.safe_body());

    for path in [
        "/v1/config",
        "/v1/scripts",
        "/v1/envs",
        "/v1/runs",
        "/v1/queue/stats",
        "/v1/batteries",
    ] {
        let missing = server.get_unauthenticated(path);
        assert_eq!(
            missing.status,
            401,
            "{path} missing token; body: {}",
            missing.safe_body()
        );
        assert_error_code(&missing.json(), "unauthorized");

        let invalid = server.get_with_bearer(path, "definitely-not-the-api-token-value");
        assert_eq!(
            invalid.status,
            401,
            "{path} invalid token; body: {}",
            invalid.safe_body()
        );
        assert_error_code(&invalid.json(), "unauthorized");
    }
}

fn rewrite_battery_git_url(workspace: &Path, name: &str, git_url: &str) {
    let registry_path = workspace.join(".omakure").join("batteries.json");
    let raw = fs::read_to_string(&registry_path).expect("read batteries registry");
    let mut registry: Value = serde_json::from_str(&raw).expect("parse batteries registry");
    let batteries = registry["batteries"]
        .as_array_mut()
        .expect("batteries array");
    let battery = batteries
        .iter_mut()
        .find(|entry| entry["name"] == name)
        .expect("battery entry");
    battery["git_url"] = json!(git_url);
    fs::write(
        &registry_path,
        serde_json::to_string_pretty(&registry).expect("serialize registry"),
    )
    .expect("write batteries registry");
}

fn rewrite_battery_git_url_to_https(workspace: &Path, name: &str) {
    rewrite_battery_git_url(
        workspace,
        name,
        &format!("https://example.invalid/{name}.git"),
    );
}

fn write_secret_echo_script(workspace: &Path, name: &str, default: Option<&str>) {
    let default_line = default
        .map(|value| format!(r#", "Default":"{value}""#))
        .unwrap_or_default();
    let script = workspace.join(name);
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {{
#   "Name": "secret_echo",
#   "Description": "secret HTTP e2e fixture",
#   "Fields": [
#     {{"Name":"TOKEN","Prompt":"Token","Type":"secret","Required":true,"Arg":"--token"{default_line}}}
#   ]
# }}
# OMAKURE_SCHEMA_END
if [ "$2" = "$OMAKURE_EXPECTED_TOKEN" ]; then
  printf 'script-saw-redacted-ok\n'
else
  printf 'secret mismatch\n' >&2
  exit 42
fi
"#
        ),
    )
    .expect("write secret script");
    support::set_executable(&script);
}

fn omakure(workspace: &Path, args: &[&str]) -> Output {
    omakure_with_env(workspace, args, &[])
}

fn omakure_with_env(workspace: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = support::omakure_command();
    command.arg("--scripts-dir").arg(workspace).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    support::command_with_timeout(&mut command, Duration::from_secs(15))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, status: {:?}, stdout_len: {}, stderr_len: {}",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len()
    );
}

fn assert_no_plaintext(output: &Output, secret: &str) {
    support::assert_no_secret_leak(&output.stdout, secret.as_bytes());
    support::assert_no_secret_leak(&output.stderr, secret.as_bytes());
}

fn assert_error_contains(envelope: &Value, needle: &str) {
    let message = envelope["error"]["message"]
        .as_str()
        .expect("error message");
    let code = envelope["error"]["code"].as_str().unwrap_or("<missing>");
    assert!(
        message.contains(needle),
        "unexpected error (code={code}, message_len={}, needle_len={})",
        message.len(),
        needle.len()
    );
}

fn assert_error_code(envelope: &Value, expected: &str) {
    assert_eq!(envelope["error"]["code"], expected);
}
