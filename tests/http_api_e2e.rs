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
///
/// Coverage-guarantee boundary: this is a DRIFT TRIPWIRE. The keys are asserted
/// to equal `HTTP_ROUTE_INVENTORY` (and that inventory is asserted equal to the
/// router's `.route(...)` registrations in `src/cli/api.rs`), so a route added
/// to the router without an inventory entry fails the suite. The
/// `Covered("test_name")` note is a human-authored pointer — it is NOT asserted
/// to reference a test that actually calls the route. A route can be "listed
/// but unexercised" if a note is added without a matching request assertion;
/// the behavioral coverage lives in the `#[test]` fns below and is kept in
/// lockstep by hand.
const HTTP_ROUTE_COVERAGE_NOTES: &[((&str, &str), RouteCoverage)] = &[
    (
        ("GET", "/v1/health"),
        RouteCoverage::Covered("health_and_config_family_routes"),
    ),
    (
        ("GET", "/v1/ready"),
        RouteCoverage::Covered("tests/node_service_e2e.rs ready_*"),
    ),
    (
        ("GET", "/v1/admin/status"),
        RouteCoverage::Covered(
            "admin_status_requires_scope_and_exposes_reload_without_secrets (unit)",
        ),
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
    (
        ("GET", "/v1/secrets"),
        RouteCoverage::Covered("secrets_metadata_endpoint_redacts_values (unit)"),
    ),
    (
        ("GET", "/v1/node/status"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("GET", "/v1/node/discovery"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("POST", "/v1/node/init"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("GET", "/v1/node/health"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("GET", "/v1/node/signals"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("POST", "/v1/node/cues"),
        RouteCoverage::Covered("node_cue_route_requires_node_write_and_a_transport"),
    ),
    (
        ("POST", "/v1/node/baselines"),
        RouteCoverage::Covered("node_baseline_route_requires_node_write_and_a_transport"),
    ),
    (
        ("POST", "/v1/node/baseline/rollback"),
        RouteCoverage::Covered("node_baseline_route_requires_node_write_and_a_transport"),
    ),
    (
        ("GET", "/v1/node/peers"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("POST", "/v1/node/peers"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("GET", "/v1/node/enrollments"),
        RouteCoverage::Covered("node_enrollment_routes"),
    ),
    (
        ("POST", "/v1/node/enrollments"),
        RouteCoverage::Covered("node_enrollment_routes"),
    ),
    (
        ("POST", "/v1/node/enrollments/:node_id/approve"),
        RouteCoverage::Covered("node_enrollment_routes"),
    ),
    (
        ("POST", "/v1/node/enrollments/:node_id/reject"),
        RouteCoverage::Covered("node_enrollment_routes"),
    ),
    (
        ("POST", "/v1/node/enrollment/bundle"),
        RouteCoverage::Covered("signed_bundle_enrollment_routes"),
    ),
    (
        ("PATCH", "/v1/node/peers/:node_id/capabilities"),
        RouteCoverage::Covered("node_management_routes"),
    ),
    (
        ("POST", "/v1/node/peers/:node_id/revoke"),
        RouteCoverage::Covered("node_management_routes"),
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

    // The HTTP search endpoint reads the FTS index without refreshing it
    // (refresh: false), so populate the index first via the CLI — the same
    // precondition a user hits after running `omakure search` once.
    let refresh = omakure(workspace.path(), &["search", "job"]);
    assert_success(&refresh);

    let search = server.get("/v1/search?q=job");
    assert_eq!(search.status, 200, "body: {}", search.safe_body());
    let search_json = search.json();
    assert!(
        search_json["data"]
            .as_array()
            .expect("search data")
            .iter()
            .any(|entry| entry["relative_path"] == "tools/job.sh"),
        "search must return the matching script, got: {}",
        search.safe_body()
    );

    // Tree root lists the `tools` directory; drilling into it lists the script.
    let tree = server.get("/v1/tree");
    assert_eq!(tree.status, 200, "body: {}", tree.safe_body());
    let tree_json = tree.json();
    assert!(
        tree_json["data"]
            .as_array()
            .expect("tree data")
            .iter()
            .any(|entry| entry["name"] == "tools" && entry["kind"] == "directory"),
        "tree root must list the tools directory, got: {}",
        tree.safe_body()
    );

    let tree_path = server.get("/v1/tree/tools");
    assert_eq!(tree_path.status, 200, "body: {}", tree_path.safe_body());
    let tree_path_json = tree_path.json();
    assert!(
        tree_path_json["data"]
            .as_array()
            .expect("tree path data")
            .iter()
            .any(|entry| entry["name"] == "job.sh" && entry["kind"] == "script"),
        "tree/tools must list job.sh as a script, got: {}",
        tree_path.safe_body()
    );
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

    // Both runs are terminal: cancelled (http-cancel-me) and dead_letter
    // (http-dead-letter). Prove each filter selects the right subset.
    let all = server.get("/v1/runs?state_set=all");
    assert_eq!(all.status, 200, "body: {}", all.safe_body());
    let all_ids = run_ids(&all.json());
    assert!(all_ids.iter().any(|id| id == "http-cancel-me"));
    assert!(all_ids.iter().any(|id| id == "http-dead-letter"));

    // in_flight excludes both terminal runs.
    let in_flight = server.get("/v1/runs?state_set=in_flight");
    assert_eq!(in_flight.status, 200, "body: {}", in_flight.safe_body());
    let in_flight_ids = run_ids(&in_flight.json());
    assert!(
        !in_flight_ids.iter().any(|id| id == "http-cancel-me")
            && !in_flight_ids.iter().any(|id| id == "http-dead-letter"),
        "in_flight must exclude terminal runs, got: {in_flight_ids:?}"
    );

    // state=cancelled selects only the cancelled run.
    let cancelled = server.get("/v1/runs?state=cancelled");
    assert_eq!(cancelled.status, 200, "body: {}", cancelled.safe_body());
    let cancelled_ids = run_ids(&cancelled.json());
    assert!(cancelled_ids.iter().any(|id| id == "http-cancel-me"));
    assert!(
        !cancelled_ids.iter().any(|id| id == "http-dead-letter"),
        "state=cancelled must exclude the dead-letter run, got: {cancelled_ids:?}"
    );

    let stats = server.get("/v1/queue/stats");
    assert_eq!(stats.status, 200, "body: {}", stats.safe_body());
    let counts = &stats.json()["data"]["counts_by_state"];
    assert!(counts.is_object());
    assert!(
        counts["cancelled"].as_u64().unwrap_or(0) >= 1,
        "queue stats must count the cancelled run (counts={counts})"
    );
    assert!(
        counts["dead_letter"].as_u64().unwrap_or(0) >= 1,
        "queue stats must count the dead-letter run (counts={counts})"
    );
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

    #[cfg(unix)]
    {
        let install = server.post_json(
            "/v1/batteries/fixture/scripts/local.echo/install",
            &json!({ "force": true }),
        );
        assert_eq!(install.status, 200, "body: {}", install.safe_body());
        assert!(workspace.path().join("scripts/echo.sh").exists());
    }
    #[cfg(not(unix))]
    {
        let install = server.post_json(
            "/v1/batteries/fixture/scripts/local.echo/install",
            &json!({ "force": true }),
        );
        assert_eq!(install.status, 409, "body: {}", install.safe_body());
        assert_error_code(&install.json(), "conflict");
    }

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

    for (method, path, body) in [
        ("GET", "/v1/config", None),
        ("GET", "/v1/scripts", None),
        ("GET", "/v1/envs", None),
        ("GET", "/v1/runs", None),
        ("GET", "/v1/queue/stats", None),
        ("GET", "/v1/batteries", None),
        ("GET", "/v1/node/status", None),
        ("POST", "/v1/node/init", Some("{}")),
        ("GET", "/v1/node/peers", None),
        ("GET", "/v1/node/enrollments", None),
        ("POST", "/v1/node/peers", Some("{}")),
        ("POST", "/v1/node/enrollments", Some("{}")),
        ("POST", "/v1/node/enrollments/omk1_test/approve", Some("{}")),
        ("POST", "/v1/node/enrollments/omk1_test/reject", Some("{}")),
        ("POST", "/v1/node/enrollment/bundle", Some("{}")),
        ("PATCH", "/v1/node/peers/omk1_test/capabilities", Some("{}")),
        ("POST", "/v1/node/peers/omk1_test/revoke", Some("{}")),
    ] {
        let body = body.map(str::to_string);
        let missing = server.request_with_auth(method, path, body.clone(), support::AuthMode::None);
        assert_eq!(
            missing.status,
            401,
            "{method} {path} missing token; body: {}",
            missing.safe_body()
        );
        assert_error_code(&missing.json(), "unauthorized");

        let invalid = server.request_with_auth(
            method,
            path,
            body,
            support::AuthMode::Bearer("definitely-not-the-api-token-value"),
        );
        assert_eq!(
            invalid.status,
            401,
            "{method} {path} invalid token; body: {}",
            invalid.safe_body()
        );
        assert_error_code(&invalid.json(), "unauthorized");
    }
}

#[test]
fn node_management_routes_cover_missing_scopes_individually() {
    let workspace = support::TestWorkspace::new("http_node_route_scopes");
    let full = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "node:read",
            "--capability",
            "node:write",
            "--capability",
            "trust:write",
            "--capability",
            "enrollment:read",
            "--capability",
            "enrollment:write",
        ],
        &[],
        Duration::from_secs(10),
    );

    let node_write_only = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "node:write"],
        &[],
        Duration::from_secs(10),
    );
    for path in [
        "/v1/node/status",
        "/v1/node/peers",
        "/v1/node/health",
        "/v1/node/signals",
    ] {
        let denied = node_write_only.request("GET", path, None);
        assert_eq!(denied.status, 403, "GET {path}: {}", denied.safe_body());
        assert_error_code(&denied.json(), "forbidden");
    }

    let node_read_only = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "node:read"],
        &[],
        Duration::from_secs(10),
    );
    let denied_init = node_read_only.post_json("/v1/node/init", &json!({}));
    assert_eq!(
        denied_init.status,
        403,
        "POST /v1/node/init: {}",
        denied_init.safe_body()
    );
    assert_error_code(&denied_init.json(), "forbidden");
    for (method, path) in [
        ("POST", "/v1/node/peers"),
        ("POST", "/v1/node/enrollment/bundle"),
        ("PATCH", "/v1/node/peers/omk1_test/capabilities"),
        ("POST", "/v1/node/peers/omk1_test/revoke"),
    ] {
        let denied = node_read_only.request(method, path, Some("{}".to_string()));
        assert_eq!(
            denied.status,
            403,
            "{method} {path}: {}",
            denied.safe_body()
        );
        assert_error_code(&denied.json(), "forbidden");
    }

    drop(node_read_only);
    drop(node_write_only);
    drop(full);
}

#[test]
fn node_management_routes_use_shared_operations_and_exact_scopes() {
    let workspace = support::TestWorkspace::new("http_node_management");
    let state = workspace.path().join("node-state");
    let config = workspace.path().join("node.toml");
    let state_string = state.to_string_lossy().to_string();
    let config_string = config.to_string_lossy().to_string();
    let envs = [
        ("OMAKURE_NODE_TEST_MODE", "1"),
        ("OMAKURE_NODE_STATE_DIR", state_string.as_str()),
        ("OMAKURE_NODE_CONFIG", config_string.as_str()),
    ];

    let readonly = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "node:read"],
        &envs,
        Duration::from_secs(10),
    );
    let readonly_init = readonly.post_json("/v1/node/init", &json!({}));
    assert_eq!(readonly_init.status, 403);
    assert_error_code(&readonly_init.json(), "forbidden");

    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "node:read",
            "--capability",
            "node:write",
            "--capability",
            "trust:write",
            "--capability",
            "enrollment:read",
            "--capability",
            "enrollment:write",
        ],
        &envs,
        Duration::from_secs(10),
    );
    let before = server.get("/v1/node/status");
    assert_eq!(before.status, 200, "body: {}", before.safe_body());
    assert_eq!(before.json()["data"]["initialized"], false);
    before.assert_no_secret("identity.key");

    let init = server.post_json("/v1/node/init", &json!({}));
    assert_eq!(init.status, 200, "body: {}", init.safe_body());
    assert_eq!(init.json()["data"]["status"]["initialized"], true);
    init.assert_no_secret("private_key");

    let node_config = fs::read_to_string(&config).expect("read node config");
    fs::write(
        &config,
        node_config.replace("enrollment = \"disabled\"", "enrollment = \"manual\""),
    )
    .expect("enable manual enrollment");
    let pending = server.get("/v1/node/enrollments");
    assert_eq!(pending.status, 200, "body: {}", pending.safe_body());
    assert!(pending.json()["data"].as_array().unwrap().is_empty());

    let staged = server.post_json(
        "/v1/node/enrollments",
        &json!({"request_hex":"00","transport_certificate":"00"}),
    );
    assert_eq!(staged.status, 400, "body: {}", staged.safe_body());
    assert_error_code(&staged.json(), "enrollment_invalid");
    let approved = server.post_json(
        "/v1/node/enrollments/omk1_test/approve",
        &json!({
            "request_hex":"00",
            "transport_certificate":"00",
            "code":"00",
            "actor":"operator",
            "reason":"test",
            "confirmed":true
        }),
    );
    assert_eq!(approved.status, 400, "body: {}", approved.safe_body());
    assert_error_code(&approved.json(), "enrollment_invalid");
    let rejected = server.post_json(
        "/v1/node/enrollments/omk1_0000000000000000000000000000000000000000000000000000000000000000/reject",
        &json!({"actor":"operator","reason":"test","confirmed":true}),
    );
    assert_eq!(rejected.status, 404, "body: {}", rejected.safe_body());
    assert_error_code(&rejected.json(), "not_found");

    let status = server.get("/v1/node/status");
    assert_eq!(status.status, 200, "body: {}", status.safe_body());
    assert_eq!(
        status.json()["data"]["identity"]["public_key"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(status.json()["data"]["identity"]
        .get("private_key")
        .is_none());

    let peer = json!({
        "node_id": "omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92",
        "public_key": "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
        "role": "performer",
        "capabilities": ["remote-run"],
        "actor": "operator",
        "reason": "approved",
        "confirmed": false
    });
    let not_confirmed = server.post_json("/v1/node/peers", &peer);
    assert_eq!(not_confirmed.status, 403);
    assert_error_code(&not_confirmed.json(), "forbidden");
    assert!(server.get("/v1/node/peers").json()["data"]
        .as_array()
        .unwrap()
        .is_empty());

    let mut confirmed_peer = peer.clone();
    confirmed_peer["confirmed"] = json!(true);
    let imported = server.post_json("/v1/node/peers", &confirmed_peer);
    assert_eq!(imported.status, 200, "body: {}", imported.safe_body());
    assert_eq!(imported.json()["data"]["state"], "active");
    imported.assert_no_secret("approved");

    let replay = server.post_json("/v1/node/peers", &confirmed_peer);
    assert_eq!(replay.status, 409, "body: {}", replay.safe_body());
    assert_error_code(&replay.json(), "conflict");
    assert_eq!(
        server.get("/v1/node/peers").json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // The Health Plane projection lists an actively trusted peer that has
    // never reported, with the frozen `unknown` presence and no Profile or
    // Pulse. Nothing a management client can send changes that: no route
    // writes health state, and the only writer is the node-to-node exchange.
    let health = server.get("/v1/node/health");
    assert_eq!(health.status, 200, "body: {}", health.safe_body());
    let body = health.json();
    assert_eq!(body["data"]["enabled"], true);
    assert_eq!(body["data"]["presence"]["unknown"], 1);
    assert_eq!(body["data"]["presence"]["online"], 0);
    assert_eq!(body["data"]["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["data"]["nodes"][0]["node_id"],
        "omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92"
    );
    assert_eq!(body["data"]["nodes"][0]["presence"], "unknown");
    // A peer that has said nothing has said nothing about a baseline either.
    // `unknown` and `none` are separate answers on this route: reading silence
    // as "holds no baseline" would put a verdict on a machine that has not
    // reported.
    assert_eq!(body["data"]["nodes"][0]["baseline_status"], "unknown");
    assert_eq!(body["data"]["baselines"]["unknown"], 1);
    assert_eq!(body["data"]["baselines"]["none"], 0);
    assert_eq!(body["data"]["baselines"]["in_sync"], 0);
    assert_eq!(body["data"]["baselines"]["drifted"], 0);
    assert_eq!(body["data"]["baselines"]["total"], 1);
    assert_eq!(body["data"]["nodes"][0]["profile"], json!(null));
    assert_eq!(body["data"]["nodes"][0]["pulse"], json!(null));
    assert_eq!(body["data"]["nodes"][0]["trust_state"], "active");
    health.assert_no_secret("identity.key");

    // The closed Signal feed is the second half of the same read surface.
    // Importing trust was an authoritative local transition, so it is visible
    // as exactly one `enrolled` Signal, bounded and newest first.
    let signals = server.get("/v1/node/signals");
    assert_eq!(signals.status, 200, "body: {}", signals.safe_body());
    let feed = signals.json();
    assert_eq!(feed["data"]["enabled"], true);
    assert_eq!(feed["data"]["gap"], false);
    assert_eq!(feed["data"]["limit"], 64);
    assert_eq!(feed["data"]["retention_seconds"], 604_800);
    let entries = feed["data"]["signals"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["kind"], "enrolled");
    assert_eq!(entries[0]["source"], "local");
    assert_eq!(
        entries[0]["subject"],
        "omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92"
    );
    assert_eq!(entries[0]["run"], json!(null));
    signals.assert_no_secret("identity.key");

    let malformed = server.patch_json(
        "/v1/node/peers/omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92/capabilities",
        &json!({"capabilities":["remote-run","notifications"],"actor":"operator","reason":"bad order","confirmed":true}),
    );
    assert_eq!(malformed.status, 400, "body: {}", malformed.safe_body());
    assert_error_code(&malformed.json(), "invalid_input");

    let update = server.patch_json(
        "/v1/node/peers/omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92/capabilities",
        &json!({"capabilities":["notifications"],"actor":"operator","reason":"narrowed","confirmed":true}),
    );
    assert_eq!(update.status, 200, "body: {}", update.safe_body());
    assert_eq!(
        update.json()["data"]["capabilities"],
        json!(["notifications"])
    );

    let revoked = server.post_json(
        "/v1/node/peers/omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92/revoke",
        &json!({"actor":"operator","reason":"retired","confirmed":true}),
    );
    assert_eq!(revoked.status, 200, "body: {}", revoked.safe_body());
    assert_eq!(revoked.json()["data"]["state"], "revoked");

    let repeated_revoke = server.post_json(
        "/v1/node/peers/omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92/revoke",
        &json!({"actor":"operator","reason":"replay","confirmed":true}),
    );
    assert_eq!(repeated_revoke.status, 409);
    assert_error_code(&repeated_revoke.json(), "conflict");

    // Revocation is immediate in the projection: a revoked peer is no longer
    // an actively trusted node and therefore no longer a fleet row.
    let after_revoke = server.get("/v1/node/health");
    assert_eq!(
        after_revoke.status,
        200,
        "body: {}",
        after_revoke.safe_body()
    );
    assert!(after_revoke.json()["data"]["nodes"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(after_revoke.json()["data"]["presence"]["total"], 0);
    assert_eq!(after_revoke.json()["data"]["baselines"]["total"], 0);

    // The local revocation Signal survives the revocation it records, which is
    // exactly what a Health Plane row keyed to the revoked peer could not do.
    let after_signals = server.get("/v1/node/signals");
    assert_eq!(after_signals.status, 200);
    let feed = after_signals.json();
    let kinds: Vec<&str> = feed["data"]["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|signal| signal["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["revoked", "enrolled"]);
    assert!(feed["data"]["cursors"].as_array().unwrap().is_empty());
}

/// The management API can read the Health Plane projection and nothing else.
///
/// This is the structural half of "no management HTTP call can substitute for
/// or forge the production node-to-node health exchange": there is exactly one
/// Health Plane route, it is a GET, and it is gated by the pre-existing
/// `node:read` capability rather than by any new scheme.
#[test]
fn no_http_route_can_write_health_plane_state() {
    let health_routes: Vec<_> = omakure::cli::api::HTTP_ROUTE_INVENTORY
        .iter()
        .filter(|(_, route)| route.contains("node/health") || route.contains("node/signals"))
        .collect();
    assert_eq!(
        health_routes,
        vec![&("GET", "/v1/node/health"), &("GET", "/v1/node/signals")],
        "the Health Plane must expose exactly two read-only management routes"
    );
}

#[test]
fn node_json_mutation_routes_return_413_envelopes_without_mutation() {
    let workspace = support::TestWorkspace::new("http_node_body_limit");
    let policy = workspace.path().join("policy.toml");
    fs::write(&policy, "version = 1\n[http]\nbody_limit_bytes = 64\n")
        .expect("write body-limit policy");
    let state = workspace.path().join("node-state");
    let config = workspace.path().join("node.toml");
    let state_string = state.to_string_lossy().to_string();
    let config_string = config.to_string_lossy().to_string();
    let envs = [
        ("OMAKURE_NODE_TEST_MODE", "1"),
        ("OMAKURE_NODE_STATE_DIR", state_string.as_str()),
        ("OMAKURE_NODE_CONFIG", config_string.as_str()),
    ];
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--policy", policy.to_str().unwrap(), "--capability", "all"],
        &envs,
        Duration::from_secs(10),
    );
    let oversized = json!({"padding": "x".repeat(256)}).to_string();

    for (method, path) in [
        ("POST", "/v1/node/init"),
        ("POST", "/v1/node/peers"),
        ("PATCH", "/v1/node/peers/omk1_test/capabilities"),
        ("POST", "/v1/node/peers/omk1_test/revoke"),
    ] {
        let response = server.request(method, path, Some(oversized.clone()));
        assert_eq!(
            response.status,
            413,
            "{method} {path}: {}",
            response.safe_body()
        );
        assert_error_code(&response.json(), "payload_too_large");
    }

    let status = server.get("/v1/node/status");
    assert_eq!(status.status, 200, "body: {}", status.safe_body());
    assert_eq!(status.json()["data"]["initialized"], false);
    assert_eq!(status.json()["data"]["trust"]["peer_count"], 0);
    assert!(
        !state.exists(),
        "oversized mutation bodies must not initialize state"
    );
}

#[test]
fn signed_bundle_route_has_its_own_body_bound() {
    let workspace = support::TestWorkspace::new("http_signed_bundle_body_limit");
    let policy = workspace.path().join("policy.toml");
    fs::write(&policy, "version = 1\n[http]\nbody_limit_bytes = 1048576\n")
        .expect("write body-limit policy");
    let state = workspace.path().join("node-state");
    let config = workspace.path().join("node.toml");
    let state_string = state.to_string_lossy().to_string();
    let config_string = config.to_string_lossy().to_string();
    let envs = [
        ("OMAKURE_NODE_TEST_MODE", "1"),
        ("OMAKURE_NODE_STATE_DIR", state_string.as_str()),
        ("OMAKURE_NODE_CONFIG", config_string.as_str()),
    ];
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--policy",
            policy.to_str().unwrap(),
            "--capability",
            "enrollment:write",
        ],
        &envs,
        Duration::from_secs(10),
    );
    let oversized = json!({
        "bundle_hex": "a".repeat(40 * 1024),
        "bootstrap_nonce": "00".repeat(16),
    })
    .to_string();
    let response = server.request("POST", "/v1/node/enrollment/bundle", Some(oversized));
    assert_eq!(response.status, 413, "body: {}", response.safe_body());
    assert_error_code(&response.json(), "payload_too_large");
    assert!(
        !state.exists(),
        "oversized bundle must not initialize state"
    );
}

#[test]
fn node_status_redacts_malformed_config_values_in_http_envelope() {
    let workspace = support::TestWorkspace::new("http_node_config_redaction");
    let state = workspace.path().join("node-state");
    let config = workspace.path().join("node.toml");
    let state_string = state.to_string_lossy().to_string();
    let config_string = config.to_string_lossy().to_string();
    let envs = [
        ("OMAKURE_NODE_TEST_MODE", "1"),
        ("OMAKURE_NODE_STATE_DIR", state_string.as_str()),
        ("OMAKURE_NODE_CONFIG", config_string.as_str()),
    ];
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--capability", "node:read", "--capability", "node:write"],
        &envs,
        Duration::from_secs(10),
    );
    let init = server.post_json("/v1/node/init", &json!({}));
    assert_eq!(init.status, 200, "body: {}", init.safe_body());

    let secret = "relay-user-super-secret-value";
    let malformed = fs::read_to_string(&config)
        .expect("read initialized node config")
        .replace("mode = \"direct\"", "mode = \"nostr\"")
        .replace(
            "relays = []",
            &format!("relays = [\"wss://user:{secret}@relay.example.test\"]"),
        );
    fs::write(&config, malformed).expect("write malformed node config");

    let response = server.get("/v1/node/status");
    assert_eq!(response.status, 500, "body: {}", response.safe_body());
    assert_error_code(&response.json(), "registry_invalid");
    assert_eq!(
        response.json()["error"]["message"],
        "node configuration is invalid or corrupt"
    );
    response.assert_no_secret(secret);
}

#[test]
fn node_cli_and_http_expose_identical_public_status_and_peers() {
    let workspace = support::TestWorkspace::new("node_adapter_parity");
    let state = workspace.path().join("node-state");
    let config = workspace.path().join("node.toml");
    let state_arg = state.to_string_lossy().to_string();
    let config_arg = config.to_string_lossy().to_string();
    let envs = [
        ("OMAKURE_NODE_TEST_MODE", "1"),
        ("OMAKURE_NODE_STATE_DIR", state_arg.as_str()),
        ("OMAKURE_NODE_CONFIG", config_arg.as_str()),
    ];
    let cli_init = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            &state_arg,
            "--node-config",
            &config_arg,
            "init",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&cli_init);
    let cli_init_json = support::json_envelope(&cli_init.stdout);
    assert_eq!(cli_init_json["data"]["status"]["initialized"], true);

    let cli_trust = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            &state_arg,
            "--node-config",
            &config_arg,
            "trust",
            "--node-id",
            "omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92",
            "--public-key",
            "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
            "--actor",
            "operator",
            "--reason",
            "parity fixture",
            "--confirmed",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&cli_trust);

    let cli_status = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            &state_arg,
            "--node-config",
            &config_arg,
            "status",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&cli_status);
    let cli_status_json = support::json_envelope(&cli_status.stdout);

    let cli_peers = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            &state_arg,
            "--node-config",
            &config_arg,
            "peers",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&cli_peers);
    let cli_peers_json = support::json_envelope(&cli_peers.stdout);

    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "node:read",
            "--capability",
            "node:write",
            "--capability",
            "trust:write",
        ],
        &envs,
        Duration::from_secs(10),
    );
    let http_status = server.get("/v1/node/status");
    assert_eq!(http_status.status, 200, "body: {}", http_status.safe_body());
    assert_eq!(http_status.json()["data"], cli_status_json["data"]);

    let http_peers = server.get("/v1/node/peers");
    assert_eq!(http_peers.status, 200, "body: {}", http_peers.safe_body());
    assert_eq!(http_peers.json()["data"], cli_peers_json["data"]);
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

/// Collect `run_id` values from a runs-list envelope so filter tests can assert
/// exact set membership (inclusion AND exclusion).
fn run_ids(envelope: &Value) -> Vec<String> {
    envelope["data"]
        .as_array()
        .expect("runs list data array")
        .iter()
        .filter_map(|row| row["run_id"].as_str().map(str::to_string))
        .collect()
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

/// Black-box coverage for tokens-file (multi-token Argon2) mode and the
/// file-mode branch of `require_scope`. The headline node-service feature — per-token
/// scopes — otherwise had only in-crate unit tests; this locks it end-to-end so
/// a regression that makes file-mode scope checks always-allow (or inverts the
/// `is_file_mode` branch, or lets the legacy env token slip through) fails here.
#[test]
fn tokens_file_mode_enforces_per_token_scopes() {
    let workspace = support::TestWorkspace::new("http_tokens_file_scopes");
    let tokens_path = workspace.path().join("tokens.toml");
    let tokens_path_str = tokens_path.to_str().expect("tokens path utf8");

    // Generate a narrowly-scoped token (config:read only) into a tokens file.
    let gen = omakure(
        workspace.path(),
        &[
            "--json",
            "token",
            "generate",
            "--id",
            "ci-readonly",
            "--scope",
            "config:read",
            "--append",
            tokens_path_str,
            "--confirmed",
        ],
    );
    assert_success(&gen);
    let config_token = support::json_envelope(&gen.stdout)["data"]["token"]
        .as_str()
        .expect("generated token plaintext")
        .to_string();

    // A SECOND token with a DISJOINT scope (scripts:read only). Two tokens with
    // non-overlapping scopes prove the enforced scope comes from the *presented*
    // token, not a fixed/first entry in the file.
    let gen2 = omakure(
        workspace.path(),
        &[
            "--json",
            "token",
            "generate",
            "--id",
            "ci-scripts",
            "--scope",
            "scripts:read",
            "--append",
            tokens_path_str,
            "--confirmed",
        ],
    );
    assert_success(&gen2);
    let scripts_token = support::json_envelope(&gen2.stdout)["data"]["token"]
        .as_str()
        .expect("generated token plaintext")
        .to_string();

    // Boot in tokens-file mode. Legacy OMAKURE_API_TOKEN is still set by the
    // harness but must be ignored once --tokens-file wins.
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &["--tokens-file", tokens_path_str],
        &[],
        Duration::from_secs(10),
    );

    // config_token: reaches /v1/config (200), denied /v1/scripts (403).
    let cfg_in = server.get_with_bearer("/v1/config", &config_token);
    assert_eq!(cfg_in.status, 200, "body: {}", cfg_in.safe_body());
    assert_eq!(cfg_in.json()["ok"], true);
    let cfg_out = server.get_with_bearer("/v1/scripts", &config_token);
    assert_eq!(
        cfg_out.status,
        403,
        "config token must not reach scripts; body: {}",
        cfg_out.safe_body()
    );
    assert_error_code(&cfg_out.json(), "forbidden");

    // scripts_token: the mirror image — reaches /v1/scripts (200), denied
    // /v1/config (403). This is what proves scopes are per-presented-token.
    let scr_in = server.get_with_bearer("/v1/scripts", &scripts_token);
    assert_eq!(scr_in.status, 200, "body: {}", scr_in.safe_body());
    assert_eq!(scr_in.json()["ok"], true);
    let scr_out = server.get_with_bearer("/v1/config", &scripts_token);
    assert_eq!(
        scr_out.status,
        403,
        "scripts token must not reach config; body: {}",
        scr_out.safe_body()
    );
    assert_error_code(&scr_out.json(), "forbidden");

    // A token absent from the file is rejected (file-mode auth actually verifies).
    let bogus = server.get_with_bearer("/v1/config", "omk_live_not_a_real_token_value_000000");
    assert_eq!(bogus.status, 401, "body: {}", bogus.safe_body());
    assert_error_code(&bogus.json(), "unauthorized");

    // The legacy env token must NOT authenticate in tokens-file mode.
    let legacy = server.get_with_bearer("/v1/config", API_TOKEN);
    assert_eq!(
        legacy.status,
        401,
        "legacy env token must not work in file mode; body: {}",
        legacy.safe_body()
    );
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

/// The Cue route is scoped like the rest of the node surface, and honest about
/// having nothing to dispatch over.
///
/// A `node:read` token must be refused, and a service with no direct transport
/// must say so rather than reporting a Cue it never sent. Both halves matter:
/// the first is the authorization boundary, the second is the difference
/// between "no session" and "refused", which a caller needs to tell apart.
#[test]
fn node_cue_route_requires_node_write_and_a_transport() {
    let workspace = support::TestWorkspace::new("node-cue-route");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(20),
    );
    let body = serde_json::json!({
        "peer_node_id": "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        "script": "deploy.sh",
        "reason": "scope check",
        "wait_seconds": 1,
    });
    let denied = server.post_json("/v1/node/cues", &body);
    assert_eq!(
        denied.status,
        403,
        "a node:read token must not dispatch: {}",
        denied.safe_body()
    );

    // A second node, because two `node serve` processes cannot share one
    // workspace: the lifecycle lock is what stops that, and rightly.
    let writer_workspace = support::TestWorkspace::new("node-cue-route-writer");
    let writer = support::HttpServer::start_node_service(
        writer_workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "node:write",
        ],
        &[],
        Duration::from_secs(20),
    );
    let without_transport = writer.post_json("/v1/node/cues", &body);
    assert_eq!(
        without_transport.status,
        400,
        "with no direct transport there is no session to carry a cue: {}",
        without_transport.safe_body()
    );

    let malformed_cue_id = serde_json::json!({
        "peer_node_id": "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        "script": "deploy.sh",
        "reason": "scope check",
        "wait_seconds": 1,
        "cue_id": "not-hex",
    });
    let malformed = writer.post_json("/v1/node/cues", &malformed_cue_id);
    assert_eq!(
        malformed.status,
        400,
        "a malformed cue_id must be refused before transport checks: {}",
        malformed.safe_body()
    );
    assert!(
        malformed.json()["error"]["code"] == "invalid_input",
        "malformed cue_id must be invalid_input: {}",
        malformed.safe_body()
    );
}

/// The baseline route is the delivery seam, and it is guarded the same way.
///
/// `node:write` decides only whether this operator may ask; every gate that
/// matters is on the receiving node. The second half is the difference between
/// "no session" and "refused" — a baseline is megabytes, and a caller that
/// could not tell those apart would retry into a node that had already said no.
#[test]
fn node_baseline_route_requires_node_write_and_a_transport() {
    let workspace = support::TestWorkspace::new("node-baseline-route");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(20),
    );
    let body = serde_json::json!({
        "peer_node_id": "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        "manifest": "00",
        "scripts": ["00"],
        "wait_seconds": 1,
    });
    let denied = server.post_json("/v1/node/baselines", &body);
    assert_eq!(
        denied.status,
        403,
        "a node:read token must not push code to a peer: {}",
        denied.safe_body()
    );

    // A second node, because two `node serve` processes cannot share one
    // workspace: the lifecycle lock is what stops that, and rightly.
    let writer_workspace = support::TestWorkspace::new("node-baseline-route-writer");
    let writer = support::HttpServer::start_node_service(
        writer_workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "node:write",
        ],
        &[],
        Duration::from_secs(20),
    );
    let without_transport = writer.post_json("/v1/node/baselines", &body);
    assert_eq!(
        without_transport.status,
        400,
        "with no direct transport there is no session to carry a baseline: {}",
        without_transport.safe_body()
    );

    // Rolling back is the one baseline act that reaches no peer, which is why
    // the second server needs no transport for it. It rides this test's two
    // servers rather than starting two more: `node serve` is the most expensive
    // fixture in this suite, and the read/write pair here is exactly the pair
    // the route needs.
    let confirmed = serde_json::json!({ "confirmed": true });
    let read_only = server.post_json("/v1/node/baseline/rollback", &confirmed);
    assert_eq!(
        read_only.status,
        403,
        "a node:read token must not replace the scripts on this machine: {}",
        read_only.safe_body()
    );

    let unconfirmed = writer.post_json(
        "/v1/node/baseline/rollback",
        &serde_json::json!({ "confirmed": false }),
    );
    assert_eq!(
        unconfirmed.status,
        403,
        "replacing every script a baseline named is said out loud: {}",
        unconfirmed.safe_body()
    );
    assert_error_code(&unconfirmed.json(), "forbidden");

    // A node that was never pushed a baseline has no previous version. Refusing
    // is the answer; a success that changed nothing would tell an operator the
    // machine had been put back when it had not.
    let nothing_to_undo = writer.post_json("/v1/node/baseline/rollback", &confirmed);
    assert_eq!(
        nothing_to_undo.status,
        404,
        "a node with no previous baseline must refuse rather than report a rollback: {}",
        nothing_to_undo.safe_body()
    );
    assert_error_code(&nothing_to_undo.json(), "not_found");
}
