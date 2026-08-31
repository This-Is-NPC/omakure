//! Real battery paired adapter probes, including HTTPS policy mismatches.

use super::{evidence, require_path, BehavioralContext};
use omakure::cli_http_parity::ProbeEvidence;
use omakure::operations::battery::{read_registry, write_registry};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const BATTERIES_DIR: &str = ".omakure/batteries";

pub const CASE_IDS: &[&str] = &[
    "exact.battery-list",
    "exact.battery-inspect",
    "exact.battery-scripts",
    "exact.battery-remove",
    "mismatch.battery-add",
    "mismatch.battery-sync",
    "mismatch.battery-install",
];

pub type Probe = fn(&BehavioralContext) -> Result<ProbeEvidence, String>;

pub fn probes() -> Vec<(&'static str, Probe)> {
    vec![
        ("exact.battery-list", battery_list),
        ("exact.battery-inspect", battery_inspect),
        ("exact.battery-scripts", battery_scripts),
        ("exact.battery-remove", battery_remove),
        ("mismatch.battery-add", battery_add_policy_mismatch),
        ("mismatch.battery-sync", battery_sync_policy_mismatch),
        ("mismatch.battery-install", battery_install_policy_mismatch),
    ]
}

const BATTERY: &str = "fixture-battery";
const SCRIPT: &str = "local.echo";

fn battery_list(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("battery_list", &["batteries:read"]);
    write_local_repository(&ctx, BATTERY);
    let add = ctx.cli(&[
        "battery",
        "add",
        ctx.fixture
            .repository
            .to_str()
            .ok_or("repository is not UTF-8")?,
        "--name",
        BATTERY,
    ]);
    if !add.status.success() {
        return Err(format!("CLI fixture registration failed: {:?}", add.status));
    }

    let cli = ctx.cli_json(&["--json", "battery", "list"]);
    let endpoint = "/v1/batteries";
    let auth = assert_auth(&ctx, "GET", endpoint, None);
    let http = ctx.http_json(ctx.server.get(endpoint));
    let cli_data = battery_list_projection(&cli);
    let http_data = battery_list_projection(&http.1);
    if cli_data.as_array().is_none_or(|items| {
        items.len() != 1
            || items[0]["name"] != BATTERY
            || items[0]["git_url"]
                != ctx
                    .fixture
                    .repository
                    .to_str()
                    .expect("repository path is UTF-8")
            || items[0]["requested_ref"] != "main"
    }) {
        return Err(format!(
            "battery list projection did not verify fixture state: {cli_data}"
        ));
    }
    if cli_data != http_data {
        return Err(format!(
            "battery list state mismatch: CLI {cli_data}, HTTP {http_data}"
        ));
    }
    let state = json!({
        "registered": true,
        "count": cli_data.as_array().map_or(0, Vec::len),
    });
    Ok(ProbeEvidence {
        cli: json!({"ok": cli["ok"], "data": {"batteries": cli_data, "identity": {"name": BATTERY}, "state": state.clone(), "auth": auth.clone()}}),
        http: json!({"ok": http.1["ok"], "data": {"batteries": http_data, "identity": {"name": BATTERY}, "state": state, "auth": auth}}),
        semantic_difference: None,
    })
}

fn battery_inspect(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("battery_inspect", &["batteries:read"]);
    seed_synced_https_battery(&ctx, BATTERY)?;

    let cli = ctx.cli_json(&["--json", "battery", "inspect", BATTERY]);
    let endpoint = &format!("/v1/batteries/{BATTERY}");
    let auth = assert_auth(&ctx, "GET", endpoint, None);
    let http = ctx.http_json(ctx.server.get(endpoint));
    let cli_data = battery_inspect_projection(&cli);
    let http_data = battery_inspect_projection(&http.1);
    if cli_data != http_data {
        return Err(format!(
            "battery inspect state mismatch: CLI {cli_data}, HTTP {http_data}"
        ));
    }
    if cli_data["name"] != BATTERY
        || cli_data["git_url"] != "https://example.invalid/fixture-battery.git"
        || cli_data["cache_status"] != "synced"
        || cli_data["resolved_commit"].is_null()
        || cli_data["manifest_name"] != BATTERY
        || cli_data["script_ids"] != json!([SCRIPT])
    {
        return Err(format!(
            "battery inspect projection did not verify identity/state: {cli_data}"
        ));
    }
    let synced = cli_data["cache_status"] == "synced" && !cli_data["resolved_commit"].is_null();
    Ok(ProbeEvidence {
        cli: json!({"ok": cli["ok"], "data": {"battery": cli_data, "identity": {"name": BATTERY}, "state": {"synced": synced}, "auth": auth.clone()}}),
        http: json!({"ok": http.1["ok"], "data": {"battery": http_data, "identity": {"name": BATTERY}, "state": {"synced": synced}, "auth": auth}}),
        semantic_difference: None,
    })
}

fn battery_scripts(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("battery_scripts", &["batteries:read"]);
    seed_synced_https_battery(&ctx, BATTERY)?;

    let cli = ctx.cli_json(&["--json", "battery", "scripts", BATTERY]);
    let endpoint = &format!("/v1/batteries/{BATTERY}/scripts");
    let auth = assert_auth(&ctx, "GET", endpoint, None);
    let http = ctx.http_json(ctx.server.get(endpoint));
    let cli_scripts = battery_scripts_projection(&cli);
    let http_scripts = battery_scripts_projection(&http.1);
    if cli_scripts != http_scripts {
        return Err(format!(
            "battery scripts state mismatch: CLI {cli_scripts}, HTTP {http_scripts}"
        ));
    }
    if cli_scripts.as_array().is_none_or(|scripts| {
        scripts.len() != 1 || scripts[0]["id"] != SCRIPT || scripts[0]["path"] != "scripts/echo.sh"
    }) {
        return Err(format!(
            "battery scripts projection did not verify script identity: {cli_scripts}"
        ));
    }
    let script_count = cli_scripts.as_array().map_or(0, Vec::len);
    let synced = script_count > 0;
    let state = json!({
        "synced": synced,
        "script_count": script_count,
    });
    Ok(ProbeEvidence {
        cli: json!({"ok": cli["ok"], "data": {"scripts": cli_scripts, "identity": {"name": BATTERY}, "state": state.clone(), "auth": auth.clone()}}),
        http: json!({"ok": http.1["ok"], "data": {"scripts": http_scripts, "identity": {"name": BATTERY}, "state": state, "auth": auth}}),
        semantic_difference: None,
    })
}

fn battery_remove(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("battery_remove", &["batteries:write", "batteries:read"]);
    write_local_repository(&ctx, BATTERY);
    add_local_battery(&ctx, BATTERY)?;
    let seeded = ctx.cli_json(&["--json", "battery", "sync", BATTERY]);
    if seeded["ok"] != true || seeded["data"]["resolved_commit"].is_null() {
        return Err(format!("CLI remove precondition sync failed: {seeded}"));
    }

    let endpoint = &format!("/v1/batteries/{BATTERY}?remove_cache=true");
    let auth = assert_auth(&ctx, "DELETE", endpoint, None);
    let cli = ctx.cli_json(&["--json", "battery", "remove", BATTERY, "--remove-cache"]);
    let cli_state = ctx.cli_json(&["--json", "battery", "list"]);
    let cli_removed = cli_state["data"]
        .as_array()
        .is_none_or(|items| items.iter().all(|item| item["name"] != BATTERY));
    if !cli_removed {
        return Err("CLI remove did not transition the registry to absent".into());
    }
    let cli_data = remove_projection(&cli);
    if cli_data["name"] != BATTERY || cli_data["cache_removed"] != true {
        return Err(format!(
            "CLI remove response did not verify identity/state: {cli_data}"
        ));
    }
    // the same initial state without manufacturing a response or mutating evidence.
    add_local_battery(&ctx, BATTERY)?;
    let reseeded = ctx.cli_json(&["--json", "battery", "sync", BATTERY]);
    if reseeded["ok"] != true || reseeded["data"]["resolved_commit"].is_null() {
        return Err(format!("HTTP remove precondition sync failed: {reseeded}"));
    }
    let http_response = ctx.server.delete(endpoint);
    if http_response.status != 200 {
        return Err(format!("HTTP remove status was {}", http_response.status));
    }
    let http_state = ctx.server.get(&format!("/v1/batteries/{BATTERY}"));
    if http_state.status != 404 {
        return Err("HTTP remove did not transition the registry to absent".into());
    }
    let http = ctx.http_json(http_response);
    let http_data = remove_projection(&http.1);
    if http_data["name"] != BATTERY || http_data["cache_removed"] != true {
        return Err(format!(
            "HTTP remove response did not verify identity/state: {http_data}"
        ));
    }
    let state = json!({"removed": true, "cache_removed": true});
    Ok(ProbeEvidence {
        cli: json!({"ok": cli["ok"], "data": {"result": cli_data, "identity": {"name": BATTERY}, "state": state.clone(), "auth": auth.clone()}}),
        http: json!({"ok": http.1["ok"], "data": {"result": http_data, "identity": {"name": BATTERY}, "state": state, "auth": auth}}),
        semantic_difference: None,
    })
}

fn battery_add_policy_mismatch(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let cli_ctx = parent.derive("battery_add_cli", &["batteries:write", "batteries:read"]);
    let http_ctx = parent.derive("battery_add_http", &["batteries:write", "batteries:read"]);
    write_local_repository(&cli_ctx, BATTERY);
    write_local_repository(&http_ctx, BATTERY);
    let cli_url = cli_ctx
        .fixture
        .repository
        .to_str()
        .ok_or("CLI repository is not UTF-8")?;
    let http_url = http_ctx
        .fixture
        .repository
        .to_str()
        .ok_or("HTTP repository is not UTF-8")?;
    let body = json!({"name": BATTERY, "git_url": http_url, "requested_ref": "main"});
    let endpoint = "/v1/batteries";
    let auth = assert_auth(&http_ctx, "POST", endpoint, Some(body.clone()));
    let cli = cli_ctx.cli_json(&["--json", "battery", "add", cli_url, "--name", BATTERY]);
    let cli_state = cli_ctx.cli_json(&["--json", "battery", "list"]);
    let cli_registered = cli_state["data"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["name"] == BATTERY))
        .is_some_and(|item| item["git_url"] == cli_url);
    if !cli_registered {
        return Err("CLI local battery add did not register the requested source".into());
    }

    let response = http_ctx.server.post_json(endpoint, &body);
    let response_body = response.json();
    if response.status != 400
        || response_body["ok"] != false
        || response_body["error"]["code"] != "invalid_input"
        || !response_body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.to_ascii_lowercase().contains("https"))
    {
        return Err(format!(
            "HTTP battery add did not reject non-HTTPS source policy: status={} body={response_body}",
            response.status
        ));
    }

    let registry_path = http_ctx
        .workspace
        .path()
        .join(".omakure")
        .join("batteries.json");
    let registry = read_registry(&registry_path).map_err(|error| error.to_string())?;
    let registry_absent = registry
        .batteries
        .iter()
        .all(|entry| entry.name != BATTERY && entry.git_url != http_url);
    if !registry_absent {
        return Err(format!(
            "HTTP rejected battery add persisted the source or name in the registry: {:?}",
            registry.batteries
        ));
    }

    let list_response = http_ctx.server.get(endpoint);
    let (list_status, list_body) = http_ctx.http_json(list_response);
    if list_status != 200 || list_body["ok"] != true {
        return Err(format!(
            "HTTP battery list after rejected add failed: status={list_status} body={list_body}"
        ));
    }
    let listed = battery_list_projection(&list_body);
    let list_items = listed
        .as_array()
        .ok_or("HTTP battery list returned a non-array payload")?;
    let list_absent = list_items
        .iter()
        .all(|entry| entry["name"] != BATTERY && entry["git_url"] != http_url);
    if !list_absent {
        return Err(format!(
            "HTTP rejected battery add persisted the source or name in the list: {listed}"
        ));
    }

    let no_mutation_state = json!({
        "source_exercised": true,
        "registry_absent": registry_absent,
        "list_absent": list_absent,
    });
    let mut paired = evidence(
        json!({"ok": cli["ok"], "data": {"identity": {"name": BATTERY, "git_url": cli_url}, "state": no_mutation_state.clone(), "auth": auth.clone()}, "mismatch": {"https_only_rejected": true}}),
        (
            response.status,
            json!({"ok": response_body["ok"], "data": {"identity": {"name": BATTERY, "git_url": http_url}, "state": no_mutation_state, "auth": auth}, "mismatch": {"https_only_rejected": true}}),
        ),
    )?;
    paired.semantic_difference = Some("battery-https-policy".into());
    Ok(paired)
}

fn battery_sync_policy_mismatch(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("battery_sync", &["batteries:write"]);
    write_local_repository(&ctx, BATTERY);
    add_local_battery(&ctx, BATTERY)?;
    let body = json!({});
    let endpoint = &format!("/v1/batteries/{BATTERY}/sync");
    let auth = assert_auth(&ctx, "POST", endpoint, Some(body.clone()));
    let cli = ctx.cli_json(&["--json", "battery", "sync", BATTERY]);
    if cli["ok"] != true
        || cli["data"]["name"] != BATTERY
        || cli["data"]["resolved_commit"].is_null()
    {
        return Err(format!("CLI local battery sync did not complete: {cli}"));
    }
    let resolved_commit = cli["data"]["resolved_commit"]
        .as_str()
        .ok_or("CLI sync did not return a resolved commit")?;
    verify_synced_cache(&ctx, BATTERY, resolved_commit)?;

    let response = ctx.server.post_json(endpoint, &body);
    let response_body = response.json();
    if response.status != 400
        || response_body["ok"] != false
        || response_body["error"]["code"] != "invalid_input"
        || !response_body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.to_ascii_lowercase().contains("https"))
    {
        return Err(format!(
            "HTTP battery sync did not reject non-HTTPS source policy: status={} body={response_body}",
            response.status
        ));
    }
    // The policy rejection must not discard the real local sync state.
    verify_synced_cache(&ctx, BATTERY, resolved_commit)?;
    let mut paired = evidence(
        json!({"ok": cli["ok"], "data": {"identity": {"name": BATTERY}, "state": {"source_exercised": true, "synced": true, "cache_git_head_verified": true, "registry_sync_state_verified": true}, "auth": auth}, "mismatch": {"https_only_rejected": true}}),
        (
            response.status,
            json!({"ok": response_body["ok"], "data": {"identity": {"name": BATTERY}, "state": {"source_exercised": true, "synced": true, "cache_git_head_verified": true, "registry_sync_state_verified": true}, "auth": auth}, "mismatch": {"https_only_rejected": true}}),
        ),
    )?;
    paired.semantic_difference = Some("battery-https-policy".into());
    Ok(paired)
}

fn battery_install_policy_mismatch(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let ctx = parent.derive("battery_install", &["batteries:write"]);
    write_local_repository(&ctx, BATTERY);
    add_local_battery(&ctx, BATTERY)?;
    let synced = ctx.cli_json(&["--json", "battery", "sync", BATTERY]);
    if synced["ok"] != true || synced["data"]["resolved_commit"].is_null() {
        return Err(format!("CLI local fixture sync failed: {synced}"));
    }
    let resolved_commit = synced["data"]["resolved_commit"]
        .as_str()
        .ok_or("CLI sync did not return a resolved commit")?;
    verify_synced_cache(&ctx, BATTERY, resolved_commit)?;

    let cli = ctx.cli_json(&["--json", "battery", "install", BATTERY, SCRIPT]);
    if cli["ok"] != true
        || cli["data"]["battery_name"] != BATTERY
        || cli["data"]["script_id"] != SCRIPT
    {
        return Err(format!("CLI local battery install did not complete: {cli}"));
    }
    verify_installed_script(&ctx)?;

    let endpoint = &format!("/v1/batteries/{BATTERY}/scripts/{SCRIPT}/install");
    let body = json!({"force": true});
    let auth = assert_auth(&ctx, "POST", endpoint, Some(body.clone()));
    let response = ctx.server.post_json(endpoint, &body);
    let response_body = response.json();
    if response.status != 400
        || response_body["ok"] != false
        || response_body["error"]["code"] != "invalid_input"
        || !response_body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.to_ascii_lowercase().contains("https"))
    {
        return Err(format!(
            "HTTP battery install did not reject non-HTTPS source policy: status={} body={response_body}",
            response.status
        ));
    }
    // The policy rejection must not discard the real local install state.
    verify_synced_cache(&ctx, BATTERY, resolved_commit)?;
    verify_installed_script(&ctx)?;
    let mut paired = evidence(
        json!({"ok": cli["ok"], "data": {"identity": {"name": BATTERY, "script": SCRIPT}, "state": {"source_exercised": true, "installed": true, "cache_git_head_verified": true, "registry_sync_state_verified": true, "script_content_verified": true}, "auth": auth}, "mismatch": {"https_only_rejected": true}}),
        (
            response.status,
            json!({"ok": response_body["ok"], "data": {"identity": {"name": BATTERY, "script": SCRIPT}, "state": {"source_exercised": true, "installed": true, "cache_git_head_verified": true, "registry_sync_state_verified": true, "script_content_verified": true}, "auth": auth}, "mismatch": {"https_only_rejected": true}}),
        ),
    )?;
    paired.semantic_difference = Some("battery-https-policy".into());
    Ok(paired)
}

fn write_local_repository(ctx: &BehavioralContext, name: &str) {
    super::support::write_local_battery_repo(
        &ctx.fixture.repository,
        name,
        "deterministic battery",
    );
}

fn add_local_battery(ctx: &BehavioralContext, name: &str) -> Result<(), String> {
    let local_url = ctx
        .fixture
        .repository
        .to_str()
        .ok_or("repository is not UTF-8")?;
    let output = ctx.cli(&["battery", "add", local_url, "--name", name]);
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("CLI battery add failed: {:?}", output.status))
    }
}
fn verify_synced_cache(
    ctx: &BehavioralContext,
    name: &str,
    expected_commit: &str,
) -> Result<(), String> {
    let cache = ctx
        .fixture
        .workspace
        .join(BATTERIES_DIR)
        .join("cache")
        .join(name);
    require_path(&cache);
    let cached_commit = git_output(&cache, &["rev-parse", "HEAD"])?;
    if cached_commit != expected_commit {
        return Err(format!(
            "battery cache commit mismatch: expected {expected_commit}, got {cached_commit}"
        ));
    }

    let registry_path = ctx
        .fixture
        .workspace
        .join(".omakure")
        .join("batteries.json");
    let registry = read_registry(&registry_path).map_err(|e| e.to_string())?;
    let summary = registry
        .batteries
        .iter()
        .find(|battery| battery.name == name)
        .ok_or("synced battery missing from registry")?;
    if summary.resolved_commit.as_deref() != Some(expected_commit)
        || summary.last_synced_at.as_deref().is_none_or(str::is_empty)
    {
        return Err(format!(
            "battery registry did not retain sync state: commit={:?}, synced_at={:?}",
            summary.resolved_commit, summary.last_synced_at
        ));
    }
    Ok(())
}

fn verify_installed_script(ctx: &BehavioralContext) -> Result<(), String> {
    let script = ctx.workspace.path().join("scripts/echo.sh");
    require_path(&script);
    let content = std::fs::read_to_string(&script)
        .map_err(|error| format!("read installed script: {error}"))?;
    if !content.contains("echo battery") {
        return Err("installed battery script did not retain fixture content".into());
    }
    Ok(())
}

fn seed_synced_https_battery(ctx: &BehavioralContext, name: &str) -> Result<(), String> {
    write_local_repository(ctx, name);
    let output = ctx.cli(&[
        "battery",
        "add",
        "https://example.invalid/fixture-battery.git",
        "--name",
        name,
    ]);
    if !output.status.success() {
        return Err(format!(
            "CLI HTTPS fixture registration failed: {:?}",
            output.status
        ));
    }

    let batteries_root = ctx.fixture.workspace.join(BATTERIES_DIR);
    let registry_path = batteries_root
        .parent()
        .ok_or("battery metadata root has no parent")?
        .join("batteries.json");
    let cache = batteries_root.join("cache").join(name);
    if let Some(parent) = cache.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create cache parent: {e}"))?;
    }
    run_git(
        &ctx.fixture.repository,
        &[
            "clone",
            "--no-hardlinks",
            ".",
            cache.to_str().ok_or("cache is not UTF-8")?,
        ],
    )?;
    let commit = git_output(&cache, &["rev-parse", "HEAD"])?;
    let mut registry = read_registry(&registry_path).map_err(|e| e.to_string())?;
    let summary = registry
        .batteries
        .iter_mut()
        .find(|battery| battery.name == name)
        .ok_or("seed battery missing from registry")?;
    summary.resolved_commit = Some(commit);
    summary.last_synced_at = Some(ctx.clock_seconds().to_string());
    write_registry(&registry_path, &registry).map_err(|e| e.to_string())?;
    Ok(())
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !output.status.success() {
        return Err(format!("git {:?} failed", args));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn battery_list_projection(value: &Value) -> Value {
    Value::Array(
        value["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| {
                json!({
                    "name": entry["name"],
                    "git_url": entry["git_url"],
                    "requested_ref": entry["requested_ref"],
                    "resolved_commit": entry["resolved_commit"],
                    "last_synced_at": entry["last_synced_at"],
                })
            })
            .collect(),
    )
}

fn battery_inspect_projection(value: &Value) -> Value {
    let data = &value["data"];
    json!({
        "name": data["summary"]["name"],
        "git_url": data["summary"]["git_url"],
        "requested_ref": data["summary"]["requested_ref"],
        "resolved_commit": data["summary"]["resolved_commit"],
        "cache_status": data["cache_status"],
        "manifest_name": data["manifest"]["battery"]["name"],
        "script_ids": data["manifest"]["scripts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|script| script["id"].as_str())
            .collect::<Vec<_>>(),
    })
}

fn battery_scripts_projection(value: &Value) -> Value {
    Value::Array(
        value["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|script| {
                json!({
                    "id": script["id"],
                    "path": script["path"],
                    "description": script["description"],
                    "tags": script["tags"],
                })
            })
            .collect(),
    )
}

fn remove_projection(value: &Value) -> Value {
    json!({
        "name": value["data"]["name"],
        "cache_removed": value["data"]["cache_removed"],
    })
}

fn assert_auth(
    ctx: &BehavioralContext,
    method: &str,
    endpoint: &str,
    body: Option<Value>,
) -> Value {
    let body_text = body.as_ref().map(|value| value.to_string());
    let unauthenticated = ctx.server.request_with_auth(
        method,
        endpoint,
        body_text.clone(),
        super::support::AuthMode::None,
    );
    assert_eq!(
        unauthenticated.status, 401,
        "unauthenticated {method} {endpoint} request was not rejected"
    );

    // A valid token on a server without the endpoint capability exercises
    // authorization separately from the missing-token authentication check.
    let denied_workspace =
        super::support::TestWorkspace::new(&format!("battery_forbidden_{}", ctx.forbidden_actor()));
    let denied_server = super::support::HttpServer::start_with_args(
        denied_workspace.path(),
        super::API_TOKEN,
        &[],
        &[],
        Duration::from_secs(10),
    );
    let forbidden = denied_server.request_with_auth(
        method,
        endpoint,
        body_text,
        super::support::AuthMode::Bearer(super::API_TOKEN),
    );
    assert_eq!(
        forbidden.status, 403,
        "insufficient-capability {method} {endpoint} request was not rejected"
    );
    json!({
        "unauthenticated_rejected": true,
        "forbidden_rejected": true,
        "actors": {
            "authorized": ctx.authorized_actor(),
            "unauthenticated": ctx.unauthenticated_actor(),
            "forbidden": ctx.forbidden_actor(),
        },
    })
}
