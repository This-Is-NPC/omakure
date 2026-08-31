//! Real core/scripts/config/search paired adapter probes.

use super::BehavioralContext;
use omakure::cli_http_parity::ProbeEvidence;
use serde_json::{json, Value};
use std::process::Output;

pub const CASE_IDS: &[&str] = &[
    "exact.doctor",
    "exact.scripts",
    "exact.describe",
    "mismatch.config",
    "mismatch.search",
];

pub type Probe = fn(&BehavioralContext) -> Result<ProbeEvidence, String>;

pub fn probes() -> Vec<(&'static str, Probe)> {
    vec![
        ("exact.doctor", exact_doctor),
        ("exact.scripts", exact_scripts),
        ("exact.describe", exact_describe),
        ("mismatch.config", mismatch_config),
        ("mismatch.search", mismatch_search),
    ]
}

fn exact_doctor(ctx: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = ctx.derive("parity_doctor", &["config:read"]);
    let script = ctx
        .workspace
        .write_schema_script("doctor.sh", "Doctor", "echo doctor");
    super::require_path(&script);

    let cli_output = ctx.cli(&["doctor"]);
    let http_response = ctx.server.get("/v1/doctor");
    let (status, http_json) = ctx.http_json(http_response);
    if status != 200 || http_json["ok"] != true {
        return Err(format!("doctor HTTP status {status}"));
    }

    let http_data = http_json
        .get("data")
        .ok_or_else(|| "doctor HTTP response has no data".to_string())?;
    let auth = assert_auth(&ctx, "/v1/doctor", "config:read");
    let cli = doctor_observation(&cli_output);
    if cli["ok"] != true {
        return Err("doctor CLI authorized result failed".into());
    }
    let http = json!({
        "ok": http_json["ok"],
        "data": doctor_data_projection(http_data),
    });
    let cli_state = doctor_state(&cli["data"]);
    let http_state = doctor_state(http_data);
    if cli_state != http_state
        || cli_state["required_dependencies_ready"] != true
        || cli_state["workspace_paths_present"] != true
        || cli_state["schemas_parseable"] != true
    {
        return Err(format!(
            "doctor state invariant failed: CLI={}, HTTP={}",
            cli_state, http_state
        ));
    }
    let mut cli = cli;
    let mut http = http;
    cli["auth"] = auth.clone();
    http["auth"] = auth;
    cli["state"] = cli_state.clone();
    http["state"] = http_state;
    Ok(ProbeEvidence {
        cli,
        http,
        semantic_difference: None,
    })
}

fn exact_scripts(ctx: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = ctx.derive("parity_scripts", &["scripts:read"]);
    let alpha = ctx
        .workspace
        .write_schema_script("alpha.sh", "Alpha", "echo alpha");
    let omega = ctx
        .workspace
        .write_schema_script("omega.sh", "Omega", "echo omega");
    super::require_path(&alpha);
    super::require_path(&omega);

    let cli = ctx.cli_json(&["--json", "scripts"]);
    let http_response = ctx.server.get("/v1/scripts");
    let (status, http) = ctx.http_json(http_response);
    if status != 200 || http["ok"] != true || cli["ok"] != true {
        return Err(format!("scripts HTTP status {status}"));
    }
    let auth = assert_auth(&ctx, "/v1/scripts", "scripts:read");
    let cli_paths = relative_paths(&cli["data"]);
    let http_paths = relative_paths(&http["data"]);
    if cli_paths != vec!["alpha.sh".to_string(), "omega.sh".to_string()] || http_paths != cli_paths
    {
        return Err(format!(
            "scripts ordering mismatch: CLI {cli_paths:?}, HTTP {http_paths:?}"
        ));
    }
    let sorted = cli_paths.windows(2).all(|pair| pair[0] < pair[1]);
    if !sorted {
        return Err(format!("scripts were not sorted: {cli_paths:?}"));
    }
    let projection = |paths: &[String]| {
        json!({
            "ok": true,
            "result": {"paths": paths, "script_count": paths.len()},
            "ordering": {"sorted_by_relative_path": sorted},
            "state": {"script_count": paths.len()},
            "auth": auth.clone(),
        })
    };
    Ok(ProbeEvidence {
        cli: projection(&cli_paths),
        http: projection(&http_paths),
        semantic_difference: None,
    })
}

fn exact_describe(ctx: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = ctx.derive("parity_describe", &["scripts:read"]);
    let script = ctx
        .workspace
        .write_schema_script("describe.sh", "Describe", "echo describe");
    super::require_path(&script);

    let cli = ctx.cli_json(&["--json", "describe", "describe.sh"]);
    let http_response = ctx.server.get("/v1/scripts/describe.sh");
    let (status, http) = ctx.http_json(http_response);
    if status != 200 || http["ok"] != true || cli["ok"] != true {
        return Err(format!("describe HTTP status {status}"));
    }
    let auth = assert_auth(&ctx, "/v1/scripts/describe.sh", "scripts:read");

    let cli_data = &cli["data"];
    let http_data = &http["data"];
    if cli_data["relative_path"] != "describe.sh"
        || http_data["relative_path"] != "describe.sh"
        || cli_data["name"] != "Describe"
        || http_data["schema"]["name"] != "Describe"
        || cli_data["description"] != http_data["schema"]["description"]
        || cli_data["tags"] != http_data["schema"]["tags"]
        || cli_data["fields"].as_array().is_none()
        || http_data["schema"]["fields"].as_array().is_none()
        || cli_data["fields"] != http_data["schema"]["fields"]
    {
        return Err("describe adapter returned an unexpected script schema".into());
    }
    let missing_cli = ctx.cli(&["--json", "describe", "missing.sh"]);
    if missing_cli.status.success() {
        return Err("CLI describe unexpectedly accepted a missing script".into());
    }
    let missing_cli_json: Value = serde_json::from_slice(&missing_cli.stdout)
        .map_err(|err| format!("parse CLI describe error: {err}"))?;
    if missing_cli_json["error"]["code"] != "not_found" {
        return Err(format!("unexpected CLI describe error: {missing_cli_json}"));
    }
    let missing_http = ctx.server.get("/v1/scripts/missing.sh");
    if missing_http.status != 404 || missing_http.json()["error"]["code"] != "not_found" {
        return Err(format!(
            "unexpected HTTP describe error: {} {}",
            missing_http.status,
            missing_http.safe_body()
        ));
    }
    let result = json!({
        "identity": {"relative_path": "describe.sh", "name": "Describe"},
        "schema": {
            "description": cli_data["description"],
            "tags": cli_data["tags"],
            "fields": cli_data["fields"],
        },
    });
    let errors = json!({"missing_script": true});
    Ok(ProbeEvidence {
        cli: json!({"ok": cli["ok"], "result": result, "errors": errors, "auth": auth.clone()}),
        http: json!({
            "ok": http["ok"],
            "result": {
                "identity": {"relative_path": http_data["relative_path"], "name": http_data["schema"]["name"]},
                "schema": {
                    "description": http_data["schema"]["description"],
                    "tags": http_data["schema"]["tags"],
                    "fields": http_data["schema"]["fields"],
                },
            },
            "errors": {"missing_script": true},
            "auth": auth,
        }),
        semantic_difference: None,
    })
}

fn mismatch_config(ctx: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = ctx.derive("parity_config", &["config:read"]);
    let created = ctx.cli(&[
        "env",
        "create",
        "parity",
        "PLAIN_VALUE=visible-value",
        "API_KEY=fixture-secret",
    ]);
    if !created.status.success() {
        return Err(format!(
            "CLI env fixture creation failed: {}",
            String::from_utf8_lossy(&created.stderr)
        ));
    }
    let activated = ctx.cli(&["env", "activate", "parity"]);
    if !activated.status.success() {
        return Err(format!(
            "CLI env fixture activation failed: {}",
            String::from_utf8_lossy(&activated.stderr)
        ));
    }

    let cli = ctx.cli_json(&["--json", "config"]);
    let http_response = ctx.server.get("/v1/config");
    let (status, http) = ctx.http_json(http_response);
    if status != 200 || cli["ok"] != true || http["ok"] != true {
        return Err(format!("config HTTP status {status}"));
    }
    let auth = assert_auth(&ctx, "/v1/config", "config:read");
    let cli_data = &cli["data"];
    let http_data = &http["data"];
    let cli_keys = &cli_data["active_env_keys"];
    let http_keys = &http_data["active_env_keys"];
    // The CLI's resolver masks sensitive keys, while the HTTP adapter masks
    // every active value. Verify both real policies and retain the key
    // presence as the CLI-side redaction observable.
    let cli_secret_present = has_env_key(cli_keys, "API_KEY");
    let cli_secret_visible = cli_secret_present;
    let cli_secret_masked = has_env_value(cli_keys, "API_KEY", "****");
    let cli_plain_visible = has_env_value(cli_keys, "PLAIN_VALUE", "visible-value");
    let http_secret_masked = has_env_value(http_keys, "API_KEY", "****");
    let http_plain_masked = has_env_value(http_keys, "PLAIN_VALUE", "****");
    let http_secret_not_leaked = !http.to_string().contains("fixture-secret");
    let values_differ = cli_keys != http_keys;
    if !cli_secret_present || !cli_secret_masked || !cli_plain_visible {
        return Err("CLI config did not expose expected active environment keys".into());
    }
    if !http_secret_masked || !http_plain_masked || !http_secret_not_leaked {
        return Err("HTTP config did not redact active environment values".into());
    }
    let active_env = cli_data["active_env"].clone();
    if active_env != "parity.conf" || http_data["active_env"] != active_env {
        return Err(format!(
            "config active environment mismatch: CLI={}, HTTP={}",
            active_env, http_data["active_env"]
        ));
    }
    let redaction = json!({
        "cli_secret_visible": cli_secret_visible,
        "cli_plain_visible": cli_plain_visible,
        "http_secret_masked": http_secret_masked,
        "http_plain_masked": http_plain_masked,
        "http_secret_not_leaked": http_secret_not_leaked,
        "values_differ": values_differ,
    });
    let state = json!({"requested_environment_active": true});
    let cli_projection = json!({
        "ok": cli["ok"],
        "result": {"active_env": active_env, "active_env_keys": cli_keys},
        "redaction": redaction.clone(),
        "state": state.clone(),
        "auth": auth.clone(),
    });
    let http_projection = json!({
        "ok": http["ok"],
        "result": {"active_env": http_data["active_env"], "active_env_keys": http_keys},
        "redaction": redaction,
        "state": state,
        "auth": auth,
    });
    Ok(ProbeEvidence {
        cli: cli_projection,
        http: http_projection,
        semantic_difference: Some("config-redaction".into()),
    })
}

fn mismatch_search(ctx: &BehavioralContext) -> Result<ProbeEvidence, String> {
    // Build an older indexed snapshot before starting the HTTP service.  The
    // service deliberately reads this snapshot without refreshing it.
    let workspace =
        super::support::TestWorkspace::new(&format!("parity_search_{}", ctx.authorized_actor()));
    let older = workspace.write_schema_script("older.sh", "Another Needle", "echo older");
    super::require_path(&older);
    let authorized_token = actor_token(ctx.authorized_actor());
    let seeded = workspace_cli_json(
        workspace.path(),
        &["--json", "search", "needle"],
        &authorized_token,
    )?;
    if seeded["ok"] != true {
        return Err(format!("search seed failed: {seeded}"));
    }
    let seeded_paths = relative_paths(&seeded["data"]);
    if seeded_paths != ["older.sh".to_string()] || !search_ordering_valid(&seeded["data"]) {
        return Err(format!(
            "search seed did not index the older match: {seeded_paths:?}"
        ));
    }
    let fresh = workspace.write_schema_script("fresh.sh", "Fresh Needle", "echo fresh");
    super::require_path(&fresh);
    let server = super::support::HttpServer::start_with_args(
        workspace.path(),
        &authorized_token,
        &["--capability", "scripts:read"],
        &[],
        std::time::Duration::from_secs(10),
    );

    let stale_http = server.get("/v1/search?q=needle");
    let (stale_status, stale_json) = ctx.http_json(stale_http);
    if stale_status != 200 || stale_json["ok"] != true {
        return Err(format!("search HTTP status {stale_status}"));
    }
    let refreshed_cli = workspace_cli_json(
        workspace.path(),
        &["--json", "search", "needle"],
        &authorized_token,
    )?;
    let cli_matches = refreshed_cli["data"].as_array().map_or(0, Vec::len);
    let http_matches = stale_json["data"].as_array().map_or(0, Vec::len);
    let cli_paths = relative_paths(&refreshed_cli["data"]);
    let http_paths = relative_paths(&stale_json["data"]);
    let cli_refreshed = cli_matches == 2
        && cli_paths.iter().any(|path| path == "fresh.sh")
        && cli_paths.iter().any(|path| path == "older.sh");
    let http_stale = http_matches == 1
        && http_paths == ["older.sh".to_string()]
        && !http_paths.iter().any(|path| path == "fresh.sh");
    let cli_sorted = search_ordering_valid(&refreshed_cli["data"]);
    if !cli_refreshed
        || !http_stale
        || !cli_sorted
        || cli_paths != ["older.sh".to_string(), "fresh.sh".to_string()]
    {
        return Err(format!(
            "search refresh/order fixture was not observed: CLI={cli_paths:?}, HTTP={http_paths:?}"
        ));
    }

    let long_query = "q=".to_string() + &"x".repeat(257);
    let long_cli = workspace_cli(
        workspace.path(),
        &["--json", "search", &"x".repeat(257)],
        &authorized_token,
    );
    let query_cli_accepted = long_cli.status.success();
    if !query_cli_accepted {
        return Err("CLI search rejected a query accepted by its local adapter".into());
    }
    let long_http = server.get(&format!("/v1/search?{long_query}"));
    let query_http_rejected =
        long_http.status == 400 && long_http.json()["error"]["code"] == "invalid_input";
    if !query_http_rejected {
        return Err(format!(
            "unexpected HTTP long-query result: {} {}",
            long_http.status,
            long_http.safe_body()
        ));
    }
    let many_tags = (0..17).map(|_| "tag=x").collect::<Vec<_>>().join("&");
    let tags_http = server.get(&format!("/v1/search?{many_tags}"));
    let tags_http_rejected =
        tags_http.status == 400 && tags_http.json()["error"]["code"] == "invalid_input";
    if !tags_http_rejected {
        return Err(format!(
            "unexpected HTTP tag-limit result: {} {}",
            tags_http.status,
            tags_http.safe_body()
        ));
    }
    let mut cli_tag_args = vec![
        "--json".to_string(),
        "search".to_string(),
        "needle".to_string(),
    ];
    for _ in 0..17 {
        cli_tag_args.extend(["--tag".to_string(), "x".to_string()]);
    }
    let cli_tag_refs = cli_tag_args.iter().map(String::as_str).collect::<Vec<_>>();
    let many_tags_cli = workspace_cli(workspace.path(), &cli_tag_refs, &authorized_token);
    let tags_cli_accepted = many_tags_cli.status.success();
    if !tags_cli_accepted {
        return Err("CLI search rejected a tag list accepted by its local adapter".into());
    }
    let forbidden_token = actor_token(ctx.forbidden_actor());
    let forbidden_server = super::support::HttpServer::start_with_args(
        workspace.path(),
        &forbidden_token,
        &[],
        &[],
        std::time::Duration::from_secs(10),
    );
    // Keep the capability-denial service on this seeded workspace too.  The
    // parent fixture server has a different workspace and cannot prove auth
    // for the live search index.
    let auth = assert_search_auth(
        &server,
        &forbidden_server,
        ctx,
        "/v1/search?q=needle",
        "scripts:read",
    );
    let pagination = json!({
        "query_cli_accepted": query_cli_accepted,
        "query_http_rejected": query_http_rejected,
        "tags_cli_accepted": tags_cli_accepted,
        "tags_http_rejected": tags_http_rejected,
    });
    let state = json!({"cli_refreshed": cli_refreshed, "http_stale": http_stale});
    let ordering = json!({"cli_sorted": cli_sorted});
    let cli_projection = json!({
        "ok": refreshed_cli["ok"],
        "result": {"matches": refreshed_cli["data"]},
        "state": state.clone(),
        "ordering": ordering.clone(),
        "pagination": pagination.clone(),
        "auth": auth.clone(),
    });
    let http_projection = json!({
        "ok": stale_json["ok"],
        "result": {"matches": stale_json["data"]},
        "state": state,
        "ordering": ordering,
        "pagination": pagination,
        "auth": auth,
    });
    Ok(ProbeEvidence {
        cli: cli_projection,
        http: http_projection,
        semantic_difference: Some("search-refresh-limits".into()),
    })
}

fn assert_search_auth(
    authorized_server: &super::support::HttpServer,
    forbidden_server: &super::support::HttpServer,
    ctx: &BehavioralContext,
    endpoint: &str,
    capability: &str,
) -> Value {
    // Authentication and capability checks use independent actor-derived
    // contexts, and both negative requests target the endpoint under test.
    let actors_configured = !ctx.authorized_actor().is_empty()
        && !ctx.unauthenticated_actor().is_empty()
        && !ctx.forbidden_actor().is_empty();
    assert!(actors_configured, "authorization actors must be configured");

    let missing = authorized_server.get_unauthenticated(endpoint);
    assert_eq!(missing.status, 401, "unauthenticated request was accepted");
    assert_eq!(missing.json()["error"]["code"], "unauthorized");

    let forbidden = forbidden_server.get_with_bearer(endpoint, &actor_token(ctx.forbidden_actor()));
    assert_eq!(
        forbidden.status, 403,
        "capability denial for {capability} was accepted"
    );

    json!({
        "unauthenticated_rejected": true,
        "forbidden_rejected": true,
        "actors": {
            "authorized": ctx.authorized_actor(),
            "unauthenticated": ctx.unauthenticated_actor(),
            "forbidden": ctx.forbidden_actor(),
            "authorized_configured": actors_configured,
            "unauthenticated_configured": actors_configured,
            "forbidden_configured": actors_configured,
        },
    })
}

fn actor_token(actor: &str) -> String {
    format!("behavioral-parity-{actor}-token-000000000000000000000000")
}

fn workspace_cli(workspace: &std::path::Path, args: &[&str], token: &str) -> Output {
    let mut command = super::support::omakure_command();
    command
        .arg("--scripts-dir")
        .arg(workspace)
        .args(args)
        .env("OMAKURE_API_TOKEN", token);
    super::support::command_with_timeout(&mut command, std::time::Duration::from_secs(10))
}

fn workspace_cli_json(
    workspace: &std::path::Path,
    args: &[&str],
    token: &str,
) -> Result<Value, String> {
    let output = workspace_cli(workspace, args, token);

    if !output.status.success() {
        return Err(format!(
            "CLI search failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(super::support::json_envelope(&output.stdout))
}
fn assert_auth(ctx: &BehavioralContext, endpoint: &str, capability: &str) -> Value {
    // Authentication and capability checks use independent contexts, and
    // both negative requests target the endpoint under test.
    let actors_configured = !ctx.authorized_actor().is_empty()
        && !ctx.unauthenticated_actor().is_empty()
        && !ctx.forbidden_actor().is_empty();
    assert!(actors_configured, "authorization actors must be configured");
    let unauthenticated_ctx = ctx.derive(
        &format!("{}_unauthenticated", ctx.unauthenticated_actor()),
        &[capability],
    );
    let missing = unauthenticated_ctx.server.get_unauthenticated(endpoint);
    assert_eq!(missing.status, 401, "unauthenticated request was accepted");
    assert_eq!(missing.json()["error"]["code"], "unauthorized");

    let forbidden_ctx = ctx.derive(&format!("{}_forbidden", ctx.forbidden_actor()), &[]);
    let forbidden = forbidden_ctx.server.get(endpoint);
    assert_eq!(forbidden.status, 403, "capability denial was accepted");
    assert_eq!(forbidden.json()["error"]["code"], "forbidden");

    json!({
        "unauthenticated_rejected": true,
        "forbidden_rejected": true,
        "actors": {
            "authorized": ctx.authorized_actor(),
            "unauthenticated": ctx.unauthenticated_actor(),
            "forbidden": ctx.forbidden_actor(),
            "authorized_configured": actors_configured,
            "unauthenticated_configured": actors_configured,
            "forbidden_configured": actors_configured,
        },
    })
}

fn relative_paths(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["relative_path"].as_str().map(str::to_string))
        .collect()
}

fn search_ordering_valid(value: &Value) -> bool {
    let Some(entries) = value.as_array() else {
        return false;
    };
    entries.windows(2).all(|pair| {
        let Some(left_name) = pair[0]["name"].as_str() else {
            return false;
        };
        let Some(right_name) = pair[1]["name"].as_str() else {
            return false;
        };
        let Some(left_path) = pair[0]["relative_path"].as_str() else {
            return false;
        };
        let Some(right_path) = pair[1]["relative_path"].as_str() else {
            return false;
        };
        (left_name, left_path) <= (right_name, right_path)
    })
}

fn has_env_key(value: &Value, key: &str) -> bool {
    value
        .as_array()
        .into_iter()
        .flatten()
        .any(|entry| entry["key"] == key)
}
fn has_env_value(value: &Value, key: &str, expected: &str) -> bool {
    value
        .as_array()
        .into_iter()
        .flatten()
        .any(|entry| entry["key"] == key && entry["value"] == expected)
}

fn doctor_observation(output: &Output) -> Value {
    let text = String::from_utf8_lossy(&output.stdout);
    let dependencies = ["git", "bash", "jq", "powershell", "python"]
        .into_iter()
        .map(|label| {
            json!({
                "label": label,
                "required": matches!(label, "git" | "bash" | "jq"),
                "ok": line_status(&text, label),
            })
        })
        .collect::<Vec<_>>();
    let workspace_paths = [
        "workspace_root",
        "omakure_dir",
        "history_dir",
        "workspace_config",
    ]
    .into_iter()
    .map(|label| json!({"label": label, "exists": line_status(&text, label)}))
    .collect::<Vec<_>>();
    let (total, parsed) = schema_counts(&text);
    json!({
        "ok": output.status.success(),
        "data": {
            "dependencies": dependencies,
            "workspace_paths": workspace_paths,
            "schemas": {"total": total, "parsed": parsed},
        }
    })
}

fn doctor_data_projection(data: &Value) -> Value {
    json!({
        "dependencies": data["dependencies"].as_array().into_iter().flatten().map(|check| json!({
            "label": check["label"],
            "required": check["required"],
            "ok": check["ok"],
        })).collect::<Vec<_>>(),
        "workspace_paths": data["workspace_paths"].as_array().into_iter().flatten().map(|path| json!({
            "label": path["label"],
            "exists": path["exists"],
        })).collect::<Vec<_>>(),
        "schemas": {"total": data["schemas"]["total"], "parsed": data["schemas"]["parsed"]},
    })
}

fn doctor_state(data: &Value) -> Value {
    let required_dependencies_ready = data["dependencies"]
        .as_array()
        .map(|dependencies| {
            !dependencies.is_empty()
                && dependencies.iter().all(|check| {
                    check["required"]
                        .as_bool()
                        .is_some_and(|required| !required || check["ok"].as_bool() == Some(true))
                })
        })
        .unwrap_or(false);
    let workspace_paths_present = data["workspace_paths"]
        .as_array()
        .map(|paths| {
            !paths.is_empty()
                && paths
                    .iter()
                    .all(|path| path["exists"].as_bool() == Some(true))
        })
        .unwrap_or(false);
    let schemas_parseable = data["schemas"]["total"]
        .as_u64()
        .zip(data["schemas"]["parsed"].as_u64())
        .is_some_and(|(total, parsed)| total > 0 && parsed == total);
    json!({
        "required_dependencies_ready": required_dependencies_ready,
        "workspace_paths_present": workspace_paths_present,
        "schemas_parseable": schemas_parseable,
    })
}

fn line_status(text: &str, label: &str) -> bool {
    text.lines()
        .find_map(|line| {
            let trimmed = line.trim_start();
            let prefix = format!("{label}: ");
            trimmed
                .strip_prefix(&prefix)
                .map(|rest| rest.starts_with("OK"))
        })
        .unwrap_or(false)
}

fn schema_counts(text: &str) -> (usize, usize) {
    let Some(line) = text
        .lines()
        .find(|line| line.trim_start().starts_with("schemas: "))
    else {
        return (0, 0);
    };
    let numbers = line
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<usize>().ok())
        .collect::<Vec<_>>();
    if line.contains("parseable") && numbers.len() >= 2 {
        (numbers[1], numbers[0])
    } else if line.contains("invalid") && numbers.len() >= 2 {
        (numbers[1], numbers[1].saturating_sub(numbers[0]))
    } else {
        (0, 0)
    }
}
