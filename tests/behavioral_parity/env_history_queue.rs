//! Real paired probes for history, queue, and managed-environment adapters.

use super::{evidence, require_path, BehavioralContext};
use omakure::cli_http_parity::ProbeEvidence;
use serde_json::{json, Value};
use std::time::Duration;

use super::support::AuthMode;

pub const CASE_IDS: &[&str] = &[
    "exact.history-list",
    "exact.history-show",
    "exact.history-traces",
    "exact.history-stats",
    "exact.queue-add",
    "exact.queue-cancel",
    "exact.queue-dead-letter",
    "exact.env-list",
    "exact.env-create",
    "exact.env-show",
    "exact.env-replace",
    "exact.env-set",
    "exact.env-remove",
    "exact.env-activate",
    "exact.env-deactivate",
    "exact.env-delete",
];

pub type Probe = fn(&BehavioralContext) -> Result<ProbeEvidence, String>;

pub fn probes() -> Vec<(&'static str, Probe)> {
    vec![
        ("exact.history-list", history_list),
        ("exact.history-show", history_show),
        ("exact.history-traces", history_traces),
        ("exact.history-stats", history_stats),
        ("exact.queue-add", queue_add),
        ("exact.queue-cancel", queue_cancel),
        ("exact.queue-dead-letter", queue_dead_letter),
        ("exact.env-list", env_list),
        ("exact.env-create", env_create),
        ("exact.env-show", env_show),
        ("exact.env-replace", env_replace),
        ("exact.env-set", env_set),
        ("exact.env-remove", env_remove),
        ("exact.env-activate", env_activate),
        ("exact.env-deactivate", env_deactivate),
        ("exact.env-delete", env_delete),
    ]
}

fn cli_json(ctx: &BehavioralContext, args: &[&str]) -> Value {
    let output = ctx.cli(args);
    super::support::json_envelope(&output.stdout)
}

fn http_response(ctx: &BehavioralContext, response: super::support::HttpResponse) -> (u16, Value) {
    ctx.http_json(response)
}

fn data(value: &Value) -> Value {
    value.get("data").cloned().unwrap_or(Value::Null)
}

fn auth_projection(
    ctx: &BehavioralContext,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let missing = ctx.server.request_with_auth(
        method,
        path,
        body.clone().map(|value| value.to_string()),
        AuthMode::None,
    );
    assert!(
        !ctx.unauthenticated_actor().is_empty(),
        "fixture unauthenticated actor is empty"
    );
    assert!(
        !ctx.forbidden_actor().is_empty(),
        "fixture forbidden actor is empty"
    );
    let denied_label = format!("parity_forbidden_{}", ctx.forbidden_actor());
    let denied_workspace = super::support::TestWorkspace::new(&denied_label);
    let denied_server = super::support::HttpServer::start_with_args(
        denied_workspace.path(),
        super::API_TOKEN,
        &[],
        &[],
        Duration::from_secs(10),
    );
    let forbidden = denied_server.request_with_auth(
        method,
        path,
        body.map(|value| value.to_string()),
        AuthMode::Bearer(super::API_TOKEN),
    );
    json!({
        "unauthenticated_rejected": missing.status == 401,
        "forbidden_rejected": forbidden.status == 403,
    })
}

fn row_projection(row: &Value, expected_run_id: Option<&str>) -> Value {
    let keys = [
        "run_id",
        "script_name",
        "args_json",
        "actor",
        "priority",
        "state",
        "timeout_ms",
        "cron_schedule_id",
        "trigger",
        "exit_code",
        "success",
        "error",
        "parent_run_id",
        "omakure_version",
    ];
    let mut out = serde_json::Map::new();
    if let Some(object) = row.as_object() {
        for key in keys {
            if let Some(value) = object.get(key) {
                out.insert(key.to_string(), value.clone());
            }
        }
        let run_id = object.get("run_id").and_then(Value::as_str);
        let started = object.get("started_at").and_then(Value::as_i64);
        let finished = object.get("finished_at").and_then(Value::as_i64);
        let duration = object.get("duration_ms").and_then(Value::as_i64);
        out.insert("run_id_present".into(), Value::Bool(run_id.is_some()));
        if let Some(expected) = expected_run_id {
            out.insert(
                "run_id_matches_expected".into(),
                Value::Bool(run_id == Some(expected)),
            );
        }
        out.insert(
            "duration_nonnegative".into(),
            Value::Bool(duration.is_some_and(|value| value >= 0)),
        );
        out.insert(
            "duration_present_integer".into(),
            Value::Bool(duration.is_some()),
        );
        out.insert(
            "time_present".into(),
            Value::Bool(started.is_some() && finished.is_some()),
        );
        out.insert(
            "time_monotonic".into(),
            Value::Bool(match (started, finished) {
                (Some(started), Some(finished)) => started <= finished,
                _ => true,
            }),
        );
    }
    Value::Object(out)
}

fn list_projection(value: &Value, expected_run_id: Option<&str>) -> Value {
    Value::Array(
        data(value)
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| row_projection(row, expected_run_id))
            .collect(),
    )
}

fn list_invariants(value: &Value, limit: usize) -> Value {
    let rows = data(value).as_array().cloned().unwrap_or_default();
    let times: Vec<_> = rows
        .iter()
        .map(|row| {
            row.get("started_at")
                .and_then(Value::as_i64)
                .or_else(|| row.get("enqueued_at").and_then(Value::as_i64))
        })
        .collect();
    let ordered_desc = times
        .windows(2)
        .all(|pair| matches!((pair[0], pair[1]), (Some(previous), Some(next)) if previous >= next));
    json!({
        "multiple_rows": rows.len() >= 2,
        "limit_respected": rows.len() <= limit,
        "ordered_desc": ordered_desc,
    })
}
fn run_ids(value: &Value) -> Vec<String> {
    data(value)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("run_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn assert_history_rows(value: &Value, expected: &[&str], label: &str) {
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "{label} failed: {value}"
    );
    let payload = data(value);
    let rows = payload
        .as_array()
        .unwrap_or_else(|| panic!("{label} data is not an array: {value}"));
    assert_eq!(rows.len(), expected.len(), "{label} row count");
    let ids = run_ids(value);
    assert_eq!(
        ids,
        expected
            .iter()
            .map(|id| (*id).to_string())
            .collect::<Vec<_>>(),
        "{label} run IDs/order"
    );
    for row in rows {
        assert!(
            row.get("run_id").and_then(Value::as_str).is_some(),
            "{label} row missing string run_id: {row}"
        );
        assert!(
            row.get("duration_ms")
                .and_then(Value::as_i64)
                .is_some_and(|duration| duration >= 0),
            "{label} row missing/noninteger/negative duration: {row}"
        );
    }
}
fn seeded_stats(value: &Value, actor: &str, label: &str) -> Value {
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "{label} failed: {value}"
    );
    let payload = data(value);
    let stats = payload
        .as_object()
        .unwrap_or_else(|| panic!("{label} data is not an object: {value}"));
    let total = stats["total"].as_i64().unwrap_or(-1);
    let queued = stats["counts_by_state"]["queued"].as_i64().unwrap_or(-1);
    let actor_count = stats["counts_by_actor"][actor].as_i64().unwrap_or(-1);
    assert_eq!(total, 1, "{label} total");
    assert_eq!(queued, 1, "{label} queued count");
    assert_eq!(actor_count, 1, "{label} actor count");
    json!({
        "total": total,
        "queued": queued,
        "actor": actor,
        "actor_count": actor_count,
        "seed_matches": total == 1 && queued == 1 && actor_count == 1,
    })
}
fn assert_history_row(value: &Value, expected_run_id: &str, label: &str) {
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "{label} failed: {value}"
    );
    let payload = data(value);
    let row = payload
        .as_object()
        .unwrap_or_else(|| panic!("{label} data is not an object: {value}"));
    assert_eq!(
        row.get("run_id").and_then(Value::as_str),
        Some(expected_run_id),
        "{label} run ID"
    );
    assert!(
        row.get("duration_ms")
            .and_then(Value::as_i64)
            .is_some_and(|duration| duration >= 0),
        "{label} missing/noninteger/negative duration: {row:?}"
    );
}

fn env_names(value: &Value) -> Vec<String> {
    data(value)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

const ENV_LIST_EXPECTED_NAMES: [&str; 2] = ["env-list-a", "env-list-b"];

fn require_env_list(value: &Value, expected: &[&str], label: &str) -> Vec<String> {
    assert_eq!(value["ok"], true, "{label} did not return ok=true: {value}");
    let payload = value
        .get("data")
        .filter(|payload| !payload.is_null())
        .unwrap_or_else(|| panic!("{label} returned a null or missing data payload: {value}"));
    let rows = payload
        .as_array()
        .unwrap_or_else(|| panic!("{label} data payload is not an array: {value}"));
    let names = rows
        .iter()
        .map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{label} row has no string name: {row}"))
                .to_string()
        })
        .collect::<Vec<_>>();
    let mut actual = names.clone();
    actual.sort_unstable();
    let mut expected = expected
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    expected.sort_unstable();
    assert_eq!(
        actual, expected,
        "{label} did not return exactly the expected environment names: {value}"
    );
    actual
}

fn assert_env_params(value: &Value, expected: &[(&str, &str)], label: &str) {
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "{label} failed: {value}"
    );
    let actual = env_params(value);
    let expected = Value::Array(
        expected
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    );
    assert_eq!(actual, expected, "{label} params");
}

fn assert_env_mutation(value: &Value, name: &str, label: &str) {
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "{label} failed: {value}"
    );
    assert_eq!(data(value)["name"], name, "{label} target identity");
}

fn assert_env_list_contains(value: &Value, name: &str, expected_active: bool, label: &str) {
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "{label} failed: {value}"
    );
    let payload = data(value);
    let rows = payload
        .as_array()
        .unwrap_or_else(|| panic!("{label} data is not an array: {value}"));
    let matches: Vec<_> = rows.iter().filter(|row| row["name"] == name).collect();
    assert_eq!(matches.len(), 1, "{label} target identity");
    assert_eq!(
        matches[0]["active"], expected_active,
        "{label} active state"
    );
}
fn assert_env_deleted(value: &Value, name: &str, label: &str) {
    assert!(
        value["ok"].as_bool().unwrap_or(false),
        "{label} failed: {value}"
    );
    assert!(
        !env_names(value).iter().any(|candidate| candidate == name),
        "{label} target still exists: {value}"
    );
}

fn trace_projection(value: &Value) -> Value {
    Value::Array(
        data(value)
            .as_array()
            .into_iter()
            .flatten()
            .map(|trace| {
                json!({
                    "sequence": trace["sequence"],
                    "timestamp": trace["timestamp"],
                    "level": trace["level"],
                    "message": trace["message"],
                })
            })
            .collect(),
    )
}

fn history_time_invariants(value: &Value) -> Value {
    let rows = data(value).as_array().cloned().unwrap_or_default();
    let timestamps: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.get("started_at").and_then(Value::as_i64),
                row.get("finished_at").and_then(Value::as_i64),
            )
        })
        .collect();
    json!({
        "present": !rows.is_empty()
            && timestamps.iter().all(|(started, finished)| started.is_some() && finished.is_some()),
        "monotonic": !rows.is_empty()
            && timestamps.iter().all(|(started, finished)| {
                matches!((started, finished), (Some(started), Some(finished)) if started <= finished)
            }),
        "duration_nonnegative": !rows.is_empty()
            && rows.iter().all(|row| {
                row.get("duration_ms")
                    .and_then(Value::as_i64)
                    .is_some_and(|duration| duration >= 0)
            }),
    })
}
fn trace_invariants(value: &Value) -> Value {
    let rows = data(value).as_array().cloned().unwrap_or_default();
    json!({
        "time_present": !rows.is_empty()
            && rows.iter().all(|row| row["timestamp"].as_i64().is_some()),
        "monotonic": !rows.is_empty()
            && rows.windows(2).all(|pair| {
                pair[0]["sequence"]
                    .as_i64()
                    .zip(pair[1]["sequence"].as_i64())
                    .is_some_and(|(left, right)| left <= right)
            }),
    })
}

fn pair(cli: Value, http: (u16, Value)) -> Result<ProbeEvidence, String> {
    evidence(cli, http)
}

fn history_list(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("history_list", &["runs:read", "runs:enqueue"]);
    let script = ctx
        .workspace
        .write_schema_script("history.sh", "history", "echo history");
    require_path(&script);
    for id in ["history-list-1", "history-list-2", "history-list-3"] {
        let queued = ctx.cli_json(&[
            "--json",
            "queue",
            "add",
            "history.sh",
            "--run-id",
            id,
            "--actor",
            ctx.authorized_actor(),
        ]);
        assert!(queued["ok"].as_bool().unwrap_or(false));
    }
    for _ in 0..3 {
        let worker = ctx.cli(&["--json", "queue", "worker", "--once"]);
        assert!(worker.status.success(), "queue worker failed: {worker:?}");
    }
    let cli = cli_json(
        &ctx,
        &[
            "--json",
            "history",
            "list",
            "--state-set",
            "all",
            "--limit",
            "2",
        ],
    );
    let http = http_response(&ctx, ctx.server.get("/v1/runs?state_set=all&limit=2"));
    let cli_all = cli_json(
        &ctx,
        &[
            "--json",
            "history",
            "list",
            "--state-set",
            "all",
            "--limit",
            "3",
        ],
    );
    let http_all = http_response(&ctx, ctx.server.get("/v1/runs?state_set=all&limit=3"));
    let expected_ids = ["history-list-3", "history-list-2", "history-list-1"];
    let expected_limited = &expected_ids[..2];
    assert_history_rows(&cli_all, &expected_ids, "history-list CLI full");
    assert_history_rows(&http_all.1, &expected_ids, "history-list HTTP full");
    assert_history_rows(&cli, expected_limited, "history-list CLI limited");
    assert_history_rows(&http.1, expected_limited, "history-list HTTP limited");
    let cli_all_ids = run_ids(&cli_all);
    let http_all_ids = run_ids(&http_all.1);
    let expected_ids = expected_ids
        .iter()
        .map(|id| (*id).to_string())
        .collect::<Vec<_>>();
    let identity = json!({
        "expected_ids": expected_ids,
        "cli_all_ids": cli_all_ids,
        "http_all_ids": http_all_ids,
        "all_ids_match": cli_all_ids == expected_ids && http_all_ids == expected_ids,
        "order_matches": cli_all_ids == http_all_ids,
    });
    let auth = auth_projection(&ctx, "GET", "/v1/runs", None);
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "rows": list_projection(&cli, None),
                "time": history_time_invariants(&cli),
                "ordering": list_invariants(&cli, 2),
                "identity": identity.clone(),
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "rows": list_projection(&http.1, None),
                    "time": history_time_invariants(&http.1),
                    "ordering": list_invariants(&http.1, 2),
                    "identity": identity,
                    "auth": auth,
                }
            }),
        ),
    )
}

fn history_show(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("history_show", &["runs:read", "runs:enqueue"]);
    let script = ctx
        .workspace
        .write_schema_script("show.sh", "show", "echo show");
    require_path(&script);
    let added = ctx.cli_json(&[
        "--json",
        "queue",
        "add",
        "show.sh",
        "--run-id",
        "history-show-1",
        "--actor",
        ctx.authorized_actor(),
    ]);
    assert!(added["ok"].as_bool().unwrap_or(false));
    let worker = ctx.cli(&["--json", "queue", "worker", "--once"]);
    assert!(worker.status.success(), "queue worker failed: {worker:?}");
    let cli = cli_json(&ctx, &["--json", "history", "show", "history-show-1"]);
    let http = http_response(&ctx, ctx.server.get("/v1/runs/history-show-1"));
    assert_history_row(&cli, "history-show-1", "history-show CLI");
    assert_history_row(&http.1, "history-show-1", "history-show HTTP");
    let auth = auth_projection(&ctx, "GET", "/v1/runs/history-show-1", None);
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "row": row_projection(&data(&cli), Some("history-show-1")),
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "row": row_projection(&data(&http.1), Some("history-show-1")),
                    "auth": auth,
                }
            }),
        ),
    )
}

fn history_traces(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("history_traces", &["runs:read", "runs:enqueue"]);
    let trace_command = format!(
        "{} trace 'trace event' --level info",
        super::support::omakure_bin().display()
    );
    let script = ctx
        .workspace
        .write_schema_script("traces.sh", "traces", &trace_command);
    require_path(&script);
    let added = ctx.cli_json(&[
        "--json",
        "queue",
        "add",
        "traces.sh",
        "--run-id",
        "history-traces-1",
        "--actor",
        ctx.authorized_actor(),
    ]);
    assert!(added["ok"].as_bool().unwrap_or(false));
    let worker = ctx.cli(&["--json", "queue", "worker", "--once"]);
    assert!(worker.status.success(), "queue worker failed: {worker:?}");
    let cli = cli_json(&ctx, &["--json", "history", "traces", "history-traces-1"]);
    let http = http_response(&ctx, ctx.server.get("/v1/runs/history-traces-1/traces"));
    let auth = auth_projection(&ctx, "GET", "/v1/runs/history-traces-1/traces", None);
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "traces": trace_projection(&cli),
                "time": trace_invariants(&cli),
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "traces": trace_projection(&http.1),
                    "time": trace_invariants(&http.1),
                    "auth": auth,
                }
            }),
        ),
    )
}

fn history_stats(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("history_stats", &["runs:read", "runs:enqueue"]);
    let script = ctx
        .workspace
        .write_schema_script("stats.sh", "stats", "echo stats");
    require_path(&script);
    let added = ctx.cli_json(&[
        "--json",
        "queue",
        "add",
        "stats.sh",
        "--run-id",
        "history-stats-1",
        "--actor",
        ctx.authorized_actor(),
    ]);
    assert!(added["ok"].as_bool().unwrap_or(false));
    let cli = cli_json(&ctx, &["--json", "history", "stats"]);
    let alias = cli_json(&ctx, &["--json", "queue", "stats"]);
    assert_eq!(data(&cli), data(&alias), "queue stats alias diverged");
    let http = http_response(&ctx, ctx.server.get("/v1/queue/stats"));
    let cli_seeded = seeded_stats(&cli, ctx.authorized_actor(), "history-stats CLI");
    let alias_seeded = seeded_stats(&alias, ctx.authorized_actor(), "queue-stats alias");
    let http_seeded = seeded_stats(&http.1, ctx.authorized_actor(), "queue-stats HTTP");
    let auth = auth_projection(&ctx, "GET", "/v1/queue/stats", None);
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "stats": data(&cli),
                "seeded": cli_seeded,
                "alias_seeded": alias_seeded,
                "alias_matches": data(&cli) == data(&alias),
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "stats": data(&http.1),
                    "seeded": http_seeded.clone(),
                    "alias_seeded": http_seeded,
                    "alias_matches": data(&http.1) == data(&cli),
                    "auth": auth,
                }
            }),
        ),
    )
}

fn queue_add(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let capabilities = &["runs:enqueue", "runs:read"];
    let cli_ctx = parent.derive("queue_add_cli", capabilities);
    let http_ctx = parent.derive("queue_add_http", capabilities);
    let cli_script = cli_ctx
        .workspace
        .write_schema_script("queue-add.sh", "queue-add", "echo add");
    let http_script =
        http_ctx
            .workspace
            .write_schema_script("queue-add.sh", "queue-add", "echo add");
    require_path(&cli_script);
    require_path(&http_script);
    let cli = cli_json(
        &cli_ctx,
        &[
            "--json",
            "queue",
            "add",
            "queue-add.sh",
            "--run-id",
            "queue-add-shared",
            "--actor",
            cli_ctx.authorized_actor(),
            "--priority",
            "4",
        ],
    );
    let http = http_response(&http_ctx, http_ctx.server.post_json("/v1/runs", &json!({
        "script": "queue-add.sh", "run_id": "queue-add-shared", "actor": http_ctx.authorized_actor(), "priority": 4
    })));
    let cli_auth = auth_projection(
        &cli_ctx,
        "POST",
        "/v1/runs",
        Some(json!({"script":"queue-add-auth","run_id":"queue-add-auth"})),
    );
    let http_auth = auth_projection(
        &http_ctx,
        "POST",
        "/v1/runs",
        Some(json!({"script":"queue-add-auth","run_id":"queue-add-auth"})),
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "row": row_projection(&data(&cli), Some("queue-add-shared")),
                "auth": cli_auth,
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "row": row_projection(&data(&http.1), Some("queue-add-shared")),
                    "auth": http_auth,
                }
            }),
        ),
    )
}

fn queue_cancel(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let capabilities = &["runs:enqueue", "runs:cancel", "runs:read"];
    let cli_ctx = parent.derive("queue_cancel_cli", capabilities);
    let http_ctx = parent.derive("queue_cancel_http", capabilities);
    for ctx in [&cli_ctx, &http_ctx] {
        let script =
            ctx.workspace
                .write_schema_script("queue-cancel.sh", "queue-cancel", "echo cancel");
        require_path(&script);
        let added = ctx.cli_json(&[
            "--json",
            "queue",
            "add",
            "queue-cancel.sh",
            "--run-id",
            "queue-cancel-shared",
            "--actor",
            ctx.authorized_actor(),
        ]);
        assert!(added["ok"].as_bool().unwrap_or(false));
    }
    let cli = cli_json(
        &cli_ctx,
        &[
            "--json",
            "queue",
            "cancel",
            "queue-cancel-shared",
            "--reason",
            "probe",
        ],
    );
    let http = http_response(
        &http_ctx,
        http_ctx.server.post_json(
            "/v1/runs/queue-cancel-shared/cancel",
            &json!({"reason":"probe"}),
        ),
    );
    let cli_show = cli_json(
        &cli_ctx,
        &["--json", "history", "show", "queue-cancel-shared"],
    );
    let http_show = http_response(
        &http_ctx,
        http_ctx.server.get("/v1/runs/queue-cancel-shared"),
    );
    let cli_auth = auth_projection(
        &cli_ctx,
        "POST",
        "/v1/runs/queue-cancel-shared/cancel",
        Some(json!({"reason":"probe"})),
    );
    let http_auth = auth_projection(
        &http_ctx,
        "POST",
        "/v1/runs/queue-cancel-shared/cancel",
        Some(json!({"reason":"probe"})),
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "row": row_projection(&data(&cli_show), Some("queue-cancel-shared")),
                "state": {"cancelled": data(&cli_show)["state"] == "cancelled"},
                "auth": cli_auth,
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "row": row_projection(&data(&http_show.1), Some("queue-cancel-shared")),
                    "state": {"cancelled": data(&http_show.1)["state"] == "cancelled"},
                    "auth": http_auth,
                }
            }),
        ),
    )
}

fn queue_dead_letter(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let capabilities = &["runs:enqueue", "runs:dead-letter", "runs:read"];
    let cli_ctx = parent.derive("queue_dead_letter_cli", capabilities);
    let http_ctx = parent.derive("queue_dead_letter_http", capabilities);
    for ctx in [&cli_ctx, &http_ctx] {
        let script = ctx
            .workspace
            .write_schema_script("queue-dead.sh", "queue-dead", "exit 7");
        require_path(&script);
        let added = ctx.cli_json(&[
            "--json",
            "queue",
            "add",
            "queue-dead.sh",
            "--run-id",
            "queue-dead-shared",
            "--actor",
            ctx.authorized_actor(),
        ]);
        assert!(added["ok"].as_bool().unwrap_or(false));
        let worker = ctx.cli(&["--json", "queue", "worker", "--once"]);
        assert!(worker.status.success(), "queue worker failed: {worker:?}");
    }
    let cli = cli_json(
        &cli_ctx,
        &[
            "--json",
            "queue",
            "dead-letter",
            "queue-dead-shared",
            "--reason",
            "probe",
        ],
    );
    let http = http_response(
        &http_ctx,
        http_ctx.server.post_json(
            "/v1/runs/queue-dead-shared/dead-letter",
            &json!({"reason":"probe"}),
        ),
    );
    let cli_show = cli_json(
        &cli_ctx,
        &["--json", "history", "show", "queue-dead-shared"],
    );
    let http_show = http_response(&http_ctx, http_ctx.server.get("/v1/runs/queue-dead-shared"));
    let cli_auth = auth_projection(
        &cli_ctx,
        "POST",
        "/v1/runs/queue-dead-shared/dead-letter",
        Some(json!({"reason":"probe"})),
    );
    let http_auth = auth_projection(
        &http_ctx,
        "POST",
        "/v1/runs/queue-dead-shared/dead-letter",
        Some(json!({"reason":"probe"})),
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "row": row_projection(&data(&cli_show), Some("queue-dead-shared")),
                "state": {"dead_letter": data(&cli_show)["state"] == "dead_letter"},
                "auth": cli_auth,
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "row": row_projection(&data(&http_show.1), Some("queue-dead-shared")),
                    "state": {"dead_letter": data(&http_show.1)["state"] == "dead_letter"},
                    "auth": http_auth,
                }
            }),
        ),
    )
}

fn env_params(value: &Value) -> Value {
    Value::Array(
        data(value)
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| json!({"key": entry["key"], "value": entry["value"]}))
            .collect(),
    )
}

fn env_list_projection(value: &Value) -> Value {
    Value::Array(
        data(value)
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| json!({"name": entry["name"], "active": entry["active"]}))
            .collect(),
    )
}
fn env_activation_state(value: &Value, target: &str, other: &str) -> Value {
    let rows = env_list_projection(value);
    let rows = rows.as_array().cloned().unwrap_or_default();
    let target_row = rows.iter().find(|row| row["name"] == target);
    let other_row = rows.iter().find(|row| row["name"] == other);
    json!({
        "target_identity": target_row.is_some(),
        "target_active": target_row.is_some_and(|row| row["active"] == true),
        "other_inactive": other_row.is_some_and(|row| row["active"] == false),
        "active_count": rows.iter().filter(|row| row["active"] == true).count(),
    })
}
fn env_list(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("env_list", &["envs:read", "envs:write"]);
    for (name, value) in [("env-list-a", "A=1"), ("env-list-b", "B=2")] {
        let created = ctx.cli_json(&["--json", "env", "create", name, value]);
        assert!(created["ok"].as_bool().unwrap_or(false));
    }
    let cli = cli_json(&ctx, &["--json", "env", "list"]);
    let http = http_response(&ctx, ctx.server.get("/v1/envs"));
    assert_eq!(http.0, 200, "env-list HTTP status was {}", http.0);
    let expected_names = ENV_LIST_EXPECTED_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    let cli_names = require_env_list(&cli, &ENV_LIST_EXPECTED_NAMES, "env-list CLI");
    let http_names = require_env_list(&http.1, &ENV_LIST_EXPECTED_NAMES, "env-list HTTP");
    let auth = auth_projection(&ctx, "GET", "/v1/envs", None);
    let cli_state = json!({
        "success": cli["ok"] == true && cli["data"].is_array(),
        "names_match": cli_names == expected_names,
    });
    let http_state = json!({
        "success": http.1["ok"] == true && http.1["data"].is_array(),
        "names_match": http_names == expected_names,
    });
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "envs": env_list_projection(&cli),
                "state": cli_state,
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "envs": env_list_projection(&http.1),
                    "state": http_state,
                    "auth": auth,
                }
            }),
        ),
    )
}

fn env_create(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("env_create", &["envs:read", "envs:write"]);
    let cli = cli_json(
        &ctx,
        &[
            "--json",
            "env",
            "create",
            "env-create-cli",
            "HOST=host",
            "TOKEN=secret",
        ],
    );
    assert!(cli["ok"].as_bool().unwrap_or(false));
    let cli_state = cli_json(&ctx, &["--json", "env", "show", "env-create-cli"]);
    let http = http_response(&ctx, ctx.server.post_json("/v1/envs", &json!({
        "name":"env-create-http", "params":[{"key":"HOST","value":"host"},{"key":"TOKEN","value":"secret"}]
    })));
    let http_state = http_response(&ctx, ctx.server.get("/v1/envs/env-create-http"));
    let auth = auth_projection(
        &ctx,
        "POST",
        "/v1/envs",
        Some(json!({"name":"env-create-auth","params":[]})),
    );
    assert_env_mutation(&cli, "env-create-cli", "env-create CLI");
    assert_env_params(
        &cli_state,
        &[("HOST", "host"), ("TOKEN", "****")],
        "env-create CLI state",
    );
    assert!(
        http.0 == 200 && http.1["ok"] == true,
        "env-create HTTP failed: {http:?}"
    );
    assert_env_params(
        &http_state.1,
        &[("HOST", "host"), ("TOKEN", "****")],
        "env-create HTTP state",
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "params": env_params(&cli_state),
                "postcondition": {"target_identity": true, "params_match": true},
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "params": env_params(&http_state.1),
                    "postcondition": {"target_identity": true, "params_match": true},
                    "auth": auth,
                }
            }),
        ),
    )
}

fn env_show(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("env_show", &["envs:read", "envs:write"]);
    let created = cli_json(
        &ctx,
        &[
            "--json",
            "env",
            "create",
            "env-show",
            "HOST=localhost",
            "TOKEN=secret",
        ],
    );
    assert!(created["ok"].as_bool().unwrap_or(false));
    let cli = cli_json(&ctx, &["--json", "env", "show", "env-show"]);
    let http = http_response(&ctx, ctx.server.get("/v1/envs/env-show"));
    let auth = auth_projection(&ctx, "GET", "/v1/envs/env-show", None);
    assert_env_mutation(&created, "env-show", "env-show create");
    assert_env_params(
        &cli,
        &[("HOST", "localhost"), ("TOKEN", "****")],
        "env-show CLI state",
    );
    assert_env_params(
        &http.1,
        &[("HOST", "localhost"), ("TOKEN", "****")],
        "env-show HTTP state",
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "params": env_params(&cli),
                "postcondition": {"target_identity": true, "params_match": true},
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "params": env_params(&http.1),
                    "postcondition": {"target_identity": true, "params_match": true},
                    "auth": auth,
                }
            }),
        ),
    )
}

fn env_replace(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("env_replace", &["envs:read", "envs:write"]);
    for name in ["env-replace-cli", "env-replace-http"] {
        let created = cli_json(&ctx, &["--json", "env", "create", name, "OLD=discard"]);
        assert_env_mutation(&created, name, "env-replace create");
        assert_env_params(
            &cli_json(&ctx, &["--json", "env", "show", name]),
            &[("OLD", "discard")],
            "env-replace baseline",
        );
    }
    let cli = cli_json(
        &ctx,
        &[
            "--json",
            "env",
            "replace",
            "env-replace-cli",
            "NEW=value",
            "PORT=443",
        ],
    );
    let cli_state = cli_json(&ctx, &["--json", "env", "show", "env-replace-cli"]);
    let http = http_response(
        &ctx,
        ctx.server.put_json(
            "/v1/envs/env-replace-http",
            &json!({
                "params":[{"key":"NEW","value":"value"},{"key":"PORT","value":"443"}]
            }),
        ),
    );
    let http_state = http_response(&ctx, ctx.server.get("/v1/envs/env-replace-http"));
    assert_env_mutation(&cli, "env-replace-cli", "env-replace CLI");
    assert_env_params(
        &cli_state,
        &[("NEW", "value"), ("PORT", "443")],
        "env-replace CLI state",
    );
    assert!(
        http.0 == 200 && http.1["ok"] == true,
        "env-replace HTTP failed: {http:?}"
    );
    assert_env_params(
        &http_state.1,
        &[("NEW", "value"), ("PORT", "443")],
        "env-replace HTTP state",
    );
    let auth = auth_projection(
        &ctx,
        "PUT",
        "/v1/envs/env-replace-http",
        Some(json!({"params":[]})),
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "params": env_params(&cli_state),
                "postcondition": {"target_identity": true, "params_match": true},
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "params": env_params(&http_state.1),
                    "postcondition": {"target_identity": true, "params_match": true},
                    "auth": auth,
                }
            }),
        ),
    )
}

fn env_set(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("env_set", &["envs:read", "envs:write"]);
    for name in ["env-set-cli", "env-set-http"] {
        let created = cli_json(&ctx, &["--json", "env", "create", name, "BASE=1"]);
        assert_env_mutation(&created, name, "env-set create");
        assert_env_params(
            &cli_json(&ctx, &["--json", "env", "show", name]),
            &[("BASE", "1")],
            "env-set baseline",
        );
    }
    let cli = cli_json(&ctx, &["--json", "env", "set", "env-set-cli", "VALUE=3"]);
    let cli_state = cli_json(&ctx, &["--json", "env", "show", "env-set-cli"]);
    assert_env_mutation(&cli, "env-set-cli", "env-set CLI");
    assert_env_params(
        &cli_state,
        &[("BASE", "1"), ("VALUE", "3")],
        "env-set CLI state",
    );
    let http_patch = http_response(
        &ctx,
        ctx.server.patch_json(
            "/v1/envs/env-set-http",
            &json!({"params":[{"key":"VALUE","value":"2"}]}),
        ),
    );
    assert!(
        http_patch.0 == 200 && http_patch.1["ok"] == true,
        "env-set PATCH failed: {http_patch:?}"
    );
    let http_patch_state = http_response(&ctx, ctx.server.get("/v1/envs/env-set-http"));
    assert_env_params(
        &http_patch_state.1,
        &[("BASE", "1"), ("VALUE", "2")],
        "env-set PATCH state",
    );
    let http_put = http_response(
        &ctx,
        ctx.server
            .put_json("/v1/envs/env-set-http/params/VALUE", &json!({"value":"3"})),
    );
    assert!(
        http_put.0 == 200 && http_put.1["ok"] == true,
        "env-set PUT failed: {http_put:?}"
    );
    let http_state = http_response(&ctx, ctx.server.get("/v1/envs/env-set-http"));
    assert_env_params(
        &http_state.1,
        &[("BASE", "1"), ("VALUE", "3")],
        "env-set HTTP state",
    );
    let auth_patch = auth_projection(
        &ctx,
        "PATCH",
        "/v1/envs/env-set-http",
        Some(json!({"params":[]})),
    );
    let auth_put = auth_projection(
        &ctx,
        "PUT",
        "/v1/envs/env-set-http/params/VALUE",
        Some(json!({"value":"3"})),
    );
    let auth = json!({
        "unauthenticated_rejected": auth_patch["unauthenticated_rejected"],
        "forbidden_rejected": auth_patch["forbidden_rejected"],
        "put_unauthenticated_rejected": auth_put["unauthenticated_rejected"],
        "put_forbidden_rejected": auth_put["forbidden_rejected"],
    });
    let routes = json!({
        "patch_exercised": true,
        "put_exercised": true,
    });
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "params": env_params(&cli_state),
                "route": routes.clone(),
                "postcondition": {"target_identity": true, "params_match": true, "patch_applied": true},
                "auth": auth.clone(),
            }
        }),
        (
            http_put.0,
            json!({
                "ok": http_put.1["ok"],
                "data": {
                    "params": env_params(&http_state.1),
                    "route": routes,
                    "postcondition": {"target_identity": true, "params_match": true, "patch_applied": true},
                    "auth": auth,
                }
            }),
        ),
    )
}
fn env_remove(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("env_remove", &["envs:read", "envs:write"]);
    for name in ["env-remove-cli", "env-remove-http"] {
        let created = cli_json(
            &ctx,
            &["--json", "env", "create", name, "KEEP=1", "REMOVE=2"],
        );
        assert_env_mutation(&created, name, "env-remove create");
        assert_env_params(
            &cli_json(&ctx, &["--json", "env", "show", name]),
            &[("KEEP", "1"), ("REMOVE", "2")],
            "env-remove baseline",
        );
    }
    let cli = cli_json(
        &ctx,
        &["--json", "env", "remove", "env-remove-cli", "REMOVE"],
    );
    let cli_state = cli_json(&ctx, &["--json", "env", "show", "env-remove-cli"]);
    let http = http_response(
        &ctx,
        ctx.server.delete("/v1/envs/env-remove-http/params/REMOVE"),
    );
    let http_state = http_response(&ctx, ctx.server.get("/v1/envs/env-remove-http"));
    assert_env_mutation(&cli, "env-remove-cli", "env-remove CLI");
    assert_env_params(&cli_state, &[("KEEP", "1")], "env-remove CLI state");
    assert!(
        http.0 == 200 && http.1["ok"] == true,
        "env-remove HTTP failed: {http:?}"
    );
    assert_env_params(&http_state.1, &[("KEEP", "1")], "env-remove HTTP state");
    let auth = auth_projection(
        &ctx,
        "DELETE",
        "/v1/envs/env-remove-http/params/REMOVE",
        None,
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "params": env_params(&cli_state),
                "postcondition": {"target_identity": true, "params_match": true, "removed": true},
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "params": env_params(&http_state.1),
                    "postcondition": {"target_identity": true, "params_match": true, "removed": true},
                    "auth": auth,
                }
            }),
        ),
    )
}

fn env_activate(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive(
        "env_activate",
        &["envs:read", "envs:write", "envs:activate"],
    );
    for name in ["env-activate-cli", "env-activate-http"] {
        let created = cli_json(&ctx, &["--json", "env", "create", name, "ACTIVE=yes"]);
        assert_env_mutation(&created, name, "env-activate create");
        assert_env_params(
            &cli_json(&ctx, &["--json", "env", "show", name]),
            &[("ACTIVE", "yes")],
            "env-activate baseline",
        );
    }
    let cli = cli_json(&ctx, &["--json", "env", "activate", "env-activate-cli"]);
    assert_env_mutation(&cli, "env-activate-cli", "env-activate CLI");
    let cli_state = cli_json(&ctx, &["--json", "env", "list"]);
    assert_env_list_contains(
        &cli_state,
        "env-activate-cli",
        true,
        "env-activate CLI target",
    );
    assert_env_list_contains(
        &cli_state,
        "env-activate-http",
        false,
        "env-activate CLI other",
    );
    let http = http_response(
        &ctx,
        ctx.server
            .post_json("/v1/envs/env-activate-http/activate", &json!({})),
    );
    assert!(
        http.0 == 200 && http.1["ok"] == true,
        "env-activate HTTP failed: {http:?}"
    );
    let http_state = http_response(&ctx, ctx.server.get("/v1/envs"));
    assert_env_list_contains(
        &http_state.1,
        "env-activate-http",
        true,
        "env-activate HTTP target",
    );
    assert_env_list_contains(
        &http_state.1,
        "env-activate-cli",
        false,
        "env-activate HTTP other",
    );
    let auth = auth_projection(
        &ctx,
        "POST",
        "/v1/envs/env-activate-http/activate",
        Some(json!({})),
    );
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "state": env_activation_state(&cli_state, "env-activate-cli", "env-activate-http"),
                "postcondition": {"target_identity": true, "active": true, "other_inactive": true},
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "state": env_activation_state(&http_state.1, "env-activate-http", "env-activate-cli"),
                    "postcondition": {"target_identity": true, "active": true, "other_inactive": true},
                    "auth": auth,
                }
            }),
        ),
    )
}
fn env_deactivate(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive(
        "env_deactivate",
        &["envs:read", "envs:write", "envs:activate"],
    );
    for name in ["env-deactivate-cli", "env-deactivate-http"] {
        let created = cli_json(&ctx, &["--json", "env", "create", name, "ACTIVE=yes"]);
        assert_env_mutation(&created, name, "env-deactivate create");
        assert_env_params(
            &cli_json(&ctx, &["--json", "env", "show", name]),
            &[("ACTIVE", "yes")],
            "env-deactivate baseline",
        );
    }
    let activated_a = cli_json(&ctx, &["--json", "env", "activate", "env-deactivate-cli"]);
    let activated_b = cli_json(&ctx, &["--json", "env", "activate", "env-deactivate-http"]);
    assert_env_mutation(
        &activated_a,
        "env-deactivate-cli",
        "env-deactivate first activate",
    );
    assert_env_mutation(
        &activated_b,
        "env-deactivate-http",
        "env-deactivate second activate",
    );
    let cli = cli_json(&ctx, &["--json", "env", "deactivate"]);
    assert!(
        cli["ok"] == true && data(&cli)["active"].is_null(),
        "env-deactivate CLI response: {cli}"
    );
    let cli_state = cli_json(&ctx, &["--json", "env", "list"]);
    let reactivated = cli_json(&ctx, &["--json", "env", "activate", "env-deactivate-http"]);
    assert_env_mutation(
        &reactivated,
        "env-deactivate-http",
        "env-deactivate HTTP setup",
    );
    let reactivated_state = cli_json(&ctx, &["--json", "env", "list"]);
    assert_env_list_contains(
        &reactivated_state,
        "env-deactivate-http",
        true,
        "env-deactivate HTTP setup target",
    );
    let http = http_response(&ctx, ctx.server.delete("/v1/envs/active"));
    let http_state = http_response(&ctx, ctx.server.get("/v1/envs"));
    assert_env_list_contains(
        &cli_state,
        "env-deactivate-cli",
        false,
        "env-deactivate CLI target",
    );
    assert_env_list_contains(
        &cli_state,
        "env-deactivate-http",
        false,
        "env-deactivate CLI other",
    );
    assert!(
        http.0 == 200 && http.1["ok"] == true,
        "env-deactivate HTTP failed: {http:?}"
    );
    assert_env_list_contains(
        &http_state.1,
        "env-deactivate-cli",
        false,
        "env-deactivate HTTP other",
    );
    assert_env_list_contains(
        &http_state.1,
        "env-deactivate-http",
        false,
        "env-deactivate HTTP target",
    );
    let active_count = |value: &Value| {
        env_list_projection(value)
            .as_array()
            .map(|rows| rows.iter().filter(|row| row["active"] == true).count())
            .unwrap_or(0)
    };
    let auth = auth_projection(&ctx, "DELETE", "/v1/envs/active", None);
    let cli_active = active_count(&cli_state);
    let http_active = active_count(&http_state.1);
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "state": {"active_count": cli_active, "deactivated": cli_active == 0},
                "postcondition": {"target_identity": true, "all_inactive": cli_active == 0},
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "state": {"active_count": http_active, "deactivated": http_active == 0},
                    "postcondition": {"target_identity": true, "all_inactive": http_active == 0},
                    "auth": auth,
                }
            }),
        ),
    )
}
fn env_delete(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("env_delete", &["envs:read", "envs:write"]);
    for name in ["env-delete-cli", "env-delete-http"] {
        let created = cli_json(&ctx, &["--json", "env", "create", name, "DELETE=yes"]);
        assert_env_mutation(&created, name, "env-delete create");
        assert_env_params(
            &cli_json(&ctx, &["--json", "env", "show", name]),
            &[("DELETE", "yes")],
            "env-delete baseline",
        );
    }
    let cli = cli_json(&ctx, &["--json", "env", "delete", "env-delete-cli"]);
    assert_env_mutation(&cli, "env-delete-cli", "env-delete CLI");
    let cli_state = cli_json(&ctx, &["--json", "env", "list"]);
    assert_env_deleted(&cli_state, "env-delete-cli", "env-delete CLI state");
    assert_env_list_contains(&cli_state, "env-delete-http", false, "env-delete CLI other");
    let http = http_response(&ctx, ctx.server.delete("/v1/envs/env-delete-http"));
    assert!(
        http.0 == 200 && http.1["ok"] == true,
        "env-delete HTTP failed: {http:?}"
    );
    let http_state = http_response(&ctx, ctx.server.get("/v1/envs"));
    assert_env_deleted(&http_state.1, "env-delete-http", "env-delete HTTP state");
    assert!(
        env_names(&http_state.1).is_empty(),
        "env-delete HTTP left an environment: {http_state:?}"
    );
    let auth = auth_projection(&ctx, "DELETE", "/v1/envs/env-delete-http", None);
    pair(
        json!({
            "ok": cli["ok"],
            "data": {
                "deleted": true,
                "postcondition": {"target_identity": true, "deleted": true, "other_preserved": true},
                "auth": auth.clone(),
            }
        }),
        (
            http.0,
            json!({
                "ok": http.1["ok"],
                "data": {
                    "deleted": true,
                    "postcondition": {"target_identity": true, "deleted": true, "other_preserved": true},
                    "auth": auth,
                }
            }),
        ),
    )
}
