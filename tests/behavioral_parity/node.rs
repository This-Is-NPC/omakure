//! Real node, baseline and enrollment paired adapter probes.

use super::{evidence, BehavioralContext};
use omakure::baseline::SignedBaselineManifest;
use omakure::baseline_push::BaselinePush;
use omakure::cli_http_parity::ProbeEvidence;
use omakure::discovery::Beacon;
use omakure::enrollment::{self, EnrollmentRole, ManualEnrollmentRequest};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use omakure::node_transport::LocalTransport;
use omakure::operations::baseline::RetainedBaseline;
use omakure::operations::node as node_ops;
use serde_json::{json, Value};
use std::fs;
use std::net::UdpSocket;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub const CASE_IDS: &[&str] = &[
    "exact.node-init",
    "exact.node-status",
    "exact.node-peers",
    "exact.node-trust",
    "exact.node-capabilities",
    "exact.node-revoke",
    "exact.node-health",
    "exact.node-signals",
    "exact.node-baseline-push",
    "exact.node-baseline-rollback",
    "exact.node-enroll-approve",
    "exact.node-enroll-reject",
    "mismatch.node-discovery",
    "mismatch.node-cue",
    "mismatch.node-enroll-request",
    "mismatch.node-enroll-apply",
];

const CAPS: &[&str] = &[
    "node:read",
    "node:write",
    "trust:write",
    "enrollment:read",
    "enrollment:write",
    "discovery:read",
];
const PEER_ID: &str = "omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92";
const PEER_KEY: &str = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

type Probe = fn(&BehavioralContext) -> Result<ProbeEvidence, String>;

pub fn probes() -> Vec<(&'static str, Probe)> {
    vec![
        ("exact.node-init", node_init),
        ("exact.node-status", node_status),
        ("exact.node-peers", node_peers),
        ("exact.node-trust", node_trust),
        ("exact.node-capabilities", node_capabilities),
        ("exact.node-revoke", node_revoke),
        ("exact.node-health", node_health),
        ("exact.node-signals", node_signals),
        ("exact.node-baseline-push", node_baseline_push),
        ("exact.node-baseline-rollback", node_baseline_rollback),
        ("exact.node-enroll-approve", node_enroll_approve),
        ("exact.node-enroll-reject", node_enroll_reject),
        ("mismatch.node-discovery", node_discovery),
        ("mismatch.node-cue", node_cue),
        ("mismatch.node-enroll-request", node_enroll_request),
        ("mismatch.node-enroll-apply", node_enroll_apply),
    ]
}

fn pair(parent: &BehavioralContext, label: &str) -> (BehavioralContext, BehavioralContext) {
    (
        parent.derive_node(&format!("{label}_cli"), CAPS),
        parent.derive_node(&format!("{label}_http"), CAPS),
    )
}

fn node_context(ctx: &BehavioralContext) -> NodeContext {
    NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(
            Some(ctx.workspace.path().join(".node-state")),
            Some(ctx.workspace.path().join("node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .expect("resolve parity node context")
}

fn node_cli_any(ctx: &BehavioralContext, tail: &[&str]) -> Value {
    let state = ctx
        .workspace
        .path()
        .join(".node-state")
        .to_string_lossy()
        .to_string();
    let config = ctx
        .workspace
        .path()
        .join("node.toml")
        .to_string_lossy()
        .to_string();
    let mut args = vec![
        "--json",
        "node",
        "--node-state-dir",
        state.as_str(),
        "--node-config",
        config.as_str(),
    ];
    args.extend_from_slice(tail);
    let output = ctx.cli_with_env(
        &args,
        &[
            ("OMAKURE_NODE_TEST_MODE", "1"),
            ("OMAKURE_NODE_STATE_DIR", state.as_str()),
            ("OMAKURE_NODE_CONFIG", config.as_str()),
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line).expect("CLI node adapter emitted no JSON envelope")
}

fn http(
    ctx: &BehavioralContext,
    response: super::support::HttpResponse,
) -> Result<(u16, Value), String> {
    let (status, body) = ctx.http_json(response);
    if status >= 500 {
        Err(format!("HTTP adapter failed with status {status}"))
    } else {
        Ok((status, body))
    }
}

fn expect_status(status: u16, expected: u16, body: &Value) -> Result<(), String> {
    if status != expected {
        return Err(format!(
            "unexpected HTTP status {status}, expected {expected}: {body}"
        ));
    }
    Ok(())
}

fn require_fixture_actors(ctx: &BehavioralContext) -> Result<(), String> {
    for (label, actor) in [
        ("authorized", ctx.authorized_actor()),
        ("unauthenticated", ctx.unauthenticated_actor()),
        ("forbidden", ctx.forbidden_actor()),
    ] {
        if actor.trim().is_empty() {
            return Err(format!("{label} fixture actor is empty"));
        }
    }
    Ok(())
}

fn assert_get_auth(ctx: &BehavioralContext, path: &str) -> Result<(), String> {
    require_fixture_actors(ctx)?;
    let unauthenticated_ctx = ctx.derive_node(
        &format!("{}_unauthenticated_get", ctx.unauthenticated_actor()),
        CAPS,
    );
    let missing = unauthenticated_ctx.server.get_unauthenticated(path);
    if missing.status != 401 {
        return Err(format!(
            "{path} accepted a missing token: {}",
            missing.status
        ));
    }
    let forbidden_ctx = ctx.derive_node(&format!("{}_forbidden_get", ctx.forbidden_actor()), &[]);
    let forbidden = forbidden_ctx.server.get(path);
    if forbidden.status != 403 {
        return Err(format!(
            "{path} accepted an insufficient token: {}",
            forbidden.status
        ));
    }
    Ok(())
}

fn assert_post_auth(ctx: &BehavioralContext, path: &str, body: &Value) -> Result<(), String> {
    require_fixture_actors(ctx)?;
    use super::support::AuthMode;
    let unauthenticated_ctx = ctx.derive_node(
        &format!("{}_unauthenticated_post", ctx.unauthenticated_actor()),
        CAPS,
    );
    let missing = unauthenticated_ctx.server.request_with_auth(
        "POST",
        path,
        Some(body.to_string()),
        AuthMode::None,
    );
    if missing.status != 401 {
        return Err(format!(
            "{path} accepted a missing token: {}",
            missing.status
        ));
    }
    let forbidden_ctx = ctx.derive_node(&format!("{}_forbidden_post", ctx.forbidden_actor()), &[]);
    let forbidden = forbidden_ctx.server.post_json(path, body);
    if forbidden.status != 403 {
        return Err(format!(
            "{path} accepted an insufficient token: {}",
            forbidden.status
        ));
    }
    Ok(())
}

fn assert_patch_auth(ctx: &BehavioralContext, path: &str, body: &Value) -> Result<(), String> {
    require_fixture_actors(ctx)?;
    use super::support::AuthMode;
    let unauthenticated_ctx = ctx.derive_node(
        &format!("{}_unauthenticated_patch", ctx.unauthenticated_actor()),
        CAPS,
    );
    let missing = unauthenticated_ctx.server.request_with_auth(
        "PATCH",
        path,
        Some(body.to_string()),
        AuthMode::None,
    );
    if missing.status != 401 {
        return Err(format!(
            "{path} accepted a missing token: {}",
            missing.status
        ));
    }
    let forbidden_ctx = ctx.derive_node(&format!("{}_forbidden_patch", ctx.forbidden_actor()), &[]);
    let forbidden = forbidden_ctx.server.request_with_auth(
        "PATCH",
        path,
        Some(body.to_string()),
        AuthMode::Bearer(super::API_TOKEN),
    );
    if forbidden.status != 403 {
        return Err(format!(
            "{path} accepted an insufficient token: {}",
            forbidden.status
        ));
    }
    Ok(())
}

fn stable_status(value: &Value) -> Value {
    let data = &value["data"];
    json!({"initialized": data["initialized"], "config": data["config"], "trust": data["trust"], "identity_present": data["identity"].is_object()})
}
fn stable_peer(value: &Value) -> Value {
    let data = &value["data"];
    json!({"state": data["state"], "role": data["role"], "capabilities": data["capabilities"], "source": data["source"], "cleanup_pending": data["cleanup_pending"]})
}
fn verify_bundle_target(bundle_hex: &str, expected_target: &str) -> Result<bool, String> {
    let bundle_bytes = enrollment::parse_hex(bundle_hex, bundle_hex.len() / 2)
        .map_err(|error| format!("decode issued enrollment bundle: {error}"))?;
    let bundle = enrollment::SignedEnrollmentBundle::decode(&bundle_bytes)
        .map_err(|error| format!("parse issued enrollment bundle: {error}"))?;
    if bundle.audience_node_id != expected_target {
        return Err(format!(
            "issued enrollment bundle targeted {}, expected {expected_target}",
            bundle.audience_node_id
        ));
    }
    Ok(bundle.audience_node_id == expected_target)
}
fn require_audience_mismatch(value: &Value, label: &str) -> Result<(), String> {
    if value["ok"] != false
        || value["error"]["code"] != "enrollment_mismatch"
        || value["error"]["message"] != "signed enrollment bundle audience does not match this node"
    {
        return Err(format!(
            "{label} accepted the wrong-target bundle or returned the wrong error: {value}"
        ));
    }
    Ok(())
}

fn require_token_untouched(
    path: &std::path::Path,
    expected: &[u8],
    label: &str,
) -> Result<(), String> {
    let actual = fs::read(path).map_err(|error| format!("{label} token read failed: {error}"))?;
    if actual != expected {
        return Err(format!(
            "{label} wrong-target apply consumed or changed its token source"
        ));
    }
    Ok(())
}
fn trust_body(ctx: &BehavioralContext) -> Value {
    json!({"node_id":PEER_ID,"public_key":PEER_KEY,"role":"performer","capabilities":["remote-run"],"actor":ctx.authorized_actor(),"reason":"deterministic-trust","confirmed":true})
}
fn trust_tail(ctx: &BehavioralContext) -> Vec<String> {
    [
        "trust",
        "--node-id",
        PEER_ID,
        "--public-key",
        PEER_KEY,
        "--actor",
        ctx.authorized_actor(),
        "--reason",
        "deterministic-trust",
        "--capability",
        "remote-run",
        "--confirmed",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}
fn node_cli_owned(ctx: &BehavioralContext, tail: Vec<String>) -> Value {
    let args = tail.iter().map(String::as_str).collect::<Vec<_>>();
    node_cli_any(ctx, &args)
}
fn named_result_data(value: &Value) -> Value {
    json!({"ok": value["ok"], "data": value["data"], "payload_present": !value["data"].is_null()})
}
fn stable_health(value: &Value) -> Value {
    let data = &value["data"];
    json!({"local_node_id": data["local_node_id"], "enabled": data["enabled"], "nodes": data["nodes"], "presence": data["presence"], "baselines": data["baselines"]})
}
fn stable_signals(value: &Value) -> Value {
    let data = &value["data"];
    json!({"local_node_id": data["local_node_id"], "enabled": data["enabled"], "gap": data["gap"], "limit": data["limit"], "retention_seconds": data["retention_seconds"], "cursors": data["cursors"], "signals": data["signals"]})
}
fn projected(value: &Value, data: Value) -> Value {
    json!({"ok": value["ok"], "data": data})
}
fn named_projected(value: &Value, data: Value) -> Value {
    json!({"ok": value["ok"], "data": data, "payload_present": !value["data"].is_null()})
}
fn require_ok(value: &Value, label: &str) -> Result<(), String> {
    if value["ok"] != true {
        return Err(format!("{label} was not successful: {value}"));
    }
    Ok(())
}
fn require_payload(value: &Value, label: &str) -> Result<(), String> {
    require_ok(value, label)?;
    if value["data"].is_null() {
        return Err(format!("{label} returned a null payload: {value}"));
    }
    Ok(())
}
fn with_http_status(value: &Value, status: u16) -> Value {
    let mut output = value.clone();
    output["http_status"] = json!(status);
    output
}

fn initialize(ctx: &BehavioralContext) -> Result<Value, String> {
    let value = node_cli_any(ctx, &["init"]);
    if value["ok"] == true || value["error"]["code"] == "conflict" {
        Ok(value)
    } else {
        Err(format!("node init failed: {value}"))
    }
}

fn edit_config(ctx: &BehavioralContext, replacements: &[(&str, String)]) -> Result<(), String> {
    let path = ctx.workspace.path().join("node.toml");
    let mut config =
        fs::read_to_string(&path).map_err(|error| format!("read node config: {error}"))?;
    for (from, to) in replacements {
        config = config.replace(from, to);
    }
    fs::write(path, config).map_err(|error| format!("write node config: {error}"))
}

fn enable_manual(ctx: &BehavioralContext) -> Result<(), String> {
    edit_config(
        ctx,
        &[(
            "enrollment = \"disabled\"",
            "enrollment = \"manual\"".into(),
        )],
    )
}

fn enrollment_material(
    ctx: &BehavioralContext,
    candidate: &BehavioralContext,
) -> Result<(String, String, String, String), String> {
    initialize(candidate)?;
    let identity = NodeIdentity::load_existing(&node_context(candidate))
        .map_err(|error| format!("load candidate identity: {error}"))?;
    let transport = LocalTransport::load_existing(&node_context(candidate), &identity)
        .map_err(|error| format!("load candidate transport: {error}"))?;
    let offer = ManualEnrollmentRequest::create(
        &identity,
        *transport.certificate().transport_public(),
        EnrollmentRole::Conductor,
        vec!["baseline-push".to_string()],
        ctx.clock_seconds(),
        3600,
    )
    .map_err(|error| format!("create enrollment request: {error}"))?;
    let node_id = offer.request.proposer_node_id.clone();
    let certificate = enrollment::hex_bytes(transport.certificate().as_bytes());
    Ok((node_id, offer.request_hex(), certificate, offer.code_hex()))
}

fn stage(ctx: &BehavioralContext, request: &str, certificate: &str) -> Result<Value, String> {
    enable_manual(ctx)?;
    node_ops::stage_manual_enrollment_hex(&node_context(ctx), request, certificate)
        .map(|peer| serde_json::to_value(peer).expect("peer serializes"))
        .map_err(|error| format!("stage enrollment: {error}"))
}

fn node_init(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli0, http0) = pair(parent, "node_init");
    let cli = fresh_uninitialized_node(cli0);
    let http_ctx = fresh_uninitialized_node(http0);
    let c = node_cli_any(&cli, &["init"]);
    require_ok(&c, "CLI node init")?;
    if c["data"]["identity_created"] != true
        || c["data"]["registry_created"] != true
        || c["data"]["status"]["initialized"] != true
        || !c["data"]["status"]["identity"].is_object()
    {
        return Err(format!("CLI node init did not create a usable node: {c}"));
    }
    assert_post_auth(&http_ctx, "/v1/node/init", &json!({}))?;
    let (status, h) = http(
        &http_ctx,
        http_ctx.server.post_json("/v1/node/init", &json!({})),
    )?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP node init")?;
    if h["data"]["identity_created"] != true
        || h["data"]["registry_created"] != true
        || h["data"]["status"]["initialized"] != true
        || !h["data"]["status"]["identity"].is_object()
    {
        return Err(format!("HTTP node init did not create a usable node: {h}"));
    }
    evidence(
        json!({"ok": c["ok"], "result_kind": c["data"]["status"]["initialized"], "data": {"initialized": c["data"]["status"]["initialized"], "identity_present": c["data"]["status"]["identity"].is_object()}}),
        (
            status,
            json!({"ok": h["ok"], "result_kind": h["data"]["status"]["initialized"], "data": {"initialized": h["data"]["status"]["initialized"], "identity_present": h["data"]["status"]["identity"].is_object()}}),
        ),
    )
}
fn node_status(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_status");
    initialize(&cli)?;
    let c = node_cli_any(&cli, &["status"]);
    require_payload(&c, "CLI node status")?;
    assert_get_auth(&http_ctx, "/v1/node/status")?;
    let (status, h) = http(&http_ctx, http_ctx.server.get("/v1/node/status"))?;
    require_payload(&h, "HTTP node status")?;
    evidence(
        projected(&c, stable_status(&c)),
        (status, projected(&h, stable_status(&h))),
    )
}
fn node_peers(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_peers");
    initialize(&cli)?;
    let c = node_cli_any(&cli, &["peers"]);
    require_payload(&c, "CLI node peers")?;
    assert_get_auth(&http_ctx, "/v1/node/peers")?;
    let (status, h) = http(&http_ctx, http_ctx.server.get("/v1/node/peers"))?;
    expect_status(status, 200, &h)?;
    require_payload(&h, "HTTP node peers")?;
    evidence(named_result_data(&c), (status, named_result_data(&h)))
}
fn node_trust(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_trust");
    initialize(&cli)?;
    let trust_args = trust_tail(&cli);
    let c = node_cli_owned(&cli, trust_args);
    require_ok(&c, "CLI trust")?;
    let body = trust_body(&http_ctx);
    assert_post_auth(&http_ctx, "/v1/node/peers", &body)?;
    let (status, h) = http(
        &http_ctx,
        http_ctx.server.post_json("/v1/node/peers", &body),
    )?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP trust")?;
    evidence(
        projected(&c, stable_peer(&c)),
        (status, projected(&h, stable_peer(&h))),
    )
}
fn node_capabilities(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_capabilities");
    initialize(&cli)?;
    let trust_args = trust_tail(&cli);
    require_ok(&node_cli_owned(&cli, trust_args), "CLI trust setup")?;
    let actor = cli.authorized_actor();
    let c = node_cli_any(
        &cli,
        &[
            "capabilities",
            PEER_ID,
            "--capability",
            "notifications",
            "--actor",
            actor,
            "--reason",
            "deterministic-capabilities",
            "--confirmed",
        ],
    );
    require_ok(&c, "CLI capabilities")?;
    let trust = trust_body(&http_ctx);
    let _ = http(
        &http_ctx,
        http_ctx.server.post_json("/v1/node/peers", &trust),
    )?;
    let body = json!({"capabilities":["notifications"],"actor":actor,"reason":"deterministic-capabilities","confirmed":true});
    let path = format!("/v1/node/peers/{PEER_ID}/capabilities");
    assert_patch_auth(&http_ctx, &path, &body)?;
    let (status, h) = http(&http_ctx, http_ctx.server.patch_json(&path, &body))?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP capabilities")?;
    evidence(
        projected(&c, stable_peer(&c)),
        (status, projected(&h, stable_peer(&h))),
    )
}
fn node_revoke(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_revoke");
    initialize(&cli)?;
    let trust_args = trust_tail(&cli);
    require_ok(&node_cli_owned(&cli, trust_args), "CLI trust setup")?;
    let actor = cli.authorized_actor();
    let c = node_cli_any(
        &cli,
        &[
            "revoke",
            PEER_ID,
            "--actor",
            actor,
            "--reason",
            "deterministic-revoke",
            "--confirmed",
        ],
    );
    require_ok(&c, "CLI revoke")?;
    let trust = trust_body(&http_ctx);
    let _ = http(
        &http_ctx,
        http_ctx.server.post_json("/v1/node/peers", &trust),
    )?;
    let body = json!({"actor":actor,"reason":"deterministic-revoke","confirmed":true});
    let path = format!("/v1/node/peers/{PEER_ID}/revoke");
    assert_post_auth(&http_ctx, &path, &body)?;
    let (status, h) = http(&http_ctx, http_ctx.server.post_json(&path, &body))?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP revoke")?;
    evidence(
        projected(&c, stable_peer(&c)),
        (status, projected(&h, stable_peer(&h))),
    )
}
fn node_health(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_health");
    initialize(&cli)?;
    let c = node_cli_any(&cli, &["health"]);
    require_payload(&c, "CLI node health")?;
    assert_get_auth(&http_ctx, "/v1/node/health")?;
    let (status, h) = http(&http_ctx, http_ctx.server.get("/v1/node/health"))?;
    expect_status(status, 200, &h)?;
    require_payload(&h, "HTTP node health")?;
    evidence(
        named_projected(&c, stable_health(&c)),
        (status, named_projected(&h, stable_health(&h))),
    )
}
fn node_signals(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_signals");
    initialize(&cli)?;
    let c = node_cli_any(&cli, &["signals"]);
    require_payload(&c, "CLI node signals")?;
    assert_get_auth(&http_ctx, "/v1/node/signals")?;
    let (status, h) = http(&http_ctx, http_ctx.server.get("/v1/node/signals"))?;
    require_payload(&h, "HTTP node signals")?;
    evidence(
        named_projected(&c, stable_signals(&c)),
        (status, named_projected(&h, stable_signals(&h))),
    )
}

fn baseline_fixture(ctx: &BehavioralContext) -> Result<(String, String, String), String> {
    initialize(ctx)?;
    let key = node_cli_any(ctx, &["baseline", "create-key"]);
    require_ok(&key, "baseline create-key")?;
    let key_id = key["data"]["key_id"]
        .as_str()
        .ok_or("missing baseline key id")?
        .to_string();
    let public_key = key["data"]["public_key"]
        .as_str()
        .ok_or("missing baseline public key")?
        .to_string();
    let a = ctx
        .workspace
        .write_schema_script("base-a.sh", "Base A", "echo base-a");
    let b = ctx
        .workspace
        .write_schema_script("base-b.sh", "Base B", "echo base-b");
    let manifest = ctx.workspace.path().join("base.omb");
    let manifest_s = manifest.to_string_lossy().to_string();
    let published = node_cli_any(
        ctx,
        &[
            "baseline",
            "publish",
            "--script",
            a.file_name().unwrap().to_str().unwrap(),
            "--script",
            b.file_name().unwrap().to_str().unwrap(),
            "--lifetime-seconds",
            "3600",
            "--out",
            &manifest_s,
        ],
    );
    require_ok(&published, "baseline publish")?;
    Ok((manifest_s, key_id, public_key))
}

fn seed_rollback_baseline(ctx: &BehavioralContext) -> Result<(), String> {
    edit_config(
        ctx,
        &[
            ("id = \"\"", "id = \"parity-org\"".into()),
            (
                "enrollment = \"disabled\"",
                "enrollment = \"manual\"".into(),
            ),
        ],
    )?;
    let (manifest, key_id, public_key) = baseline_fixture(ctx)?;
    edit_config(
        ctx,
        &[
            (
                "allow_baseline_push = false",
                "allow_baseline_push = true".into(),
            ),
            (
                "baseline_publishers = []",
                format!(
                    "baseline_publishers = [{{ key_id = \"{key_id}\", public_key = \"{public_key}\", revoked = false }}]"
                ),
            ),
        ],
    )?;
    let bodies = ["base-a.sh", "base-b.sh"]
        .into_iter()
        .map(|name| fs::read(ctx.workspace.path().join(name)).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let push = BaselinePush::encode(
        &fs::read(manifest).map_err(|error| error.to_string())?,
        &bodies,
    );
    let retained = RetainedBaseline {
        installed_at: ctx.clock_seconds() as i64,
        push,
    };
    let metadata = ctx.workspace.path().join(".omakure");
    fs::create_dir_all(&metadata).map_err(|error| error.to_string())?;
    fs::write(
        metadata.join("baseline-previous.json"),
        serde_json::to_vec(&retained).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn restart_node(ctx: BehavioralContext) -> BehavioralContext {
    restart_node_with_env(ctx, &[])
}

fn restart_node_with_env(ctx: BehavioralContext, extra_envs: &[(&str, &str)]) -> BehavioralContext {
    let BehavioralContext {
        workspace,
        repository,
        server,
        fixture,
    } = ctx;
    let _ = server.terminate();
    let mut args = Vec::with_capacity(CAPS.len() * 2);
    for capability in CAPS {
        args.extend(["--capability", *capability]);
    }
    let server = super::support::HttpServer::start_node_service(
        workspace.path(),
        super::API_TOKEN,
        &args,
        extra_envs,
        std::time::Duration::from_secs(10),
    );
    BehavioralContext {
        workspace,
        repository,
        server,
        fixture,
    }
}
fn fresh_uninitialized_node(ctx: BehavioralContext) -> BehavioralContext {
    let BehavioralContext {
        workspace,
        repository,
        server,
        fixture,
    } = ctx;
    let _ = server.terminate();
    let state = workspace.path().join(".node-state");
    let config = workspace.path().join("node.toml");
    if state.exists() {
        fs::remove_dir_all(&state).expect("remove initialized node state");
    }
    if config.exists() {
        fs::remove_file(&config).expect("remove initialized node config");
    }
    let state_env = state.to_string_lossy().to_string();
    let mut args = Vec::with_capacity(CAPS.len() * 2);
    for capability in CAPS {
        args.extend(["--capability", *capability]);
    }
    let config_env = config.to_string_lossy().to_string();
    let server = super::support::HttpServer::start_with_args(
        workspace.path(),
        super::API_TOKEN,
        &args,
        &[
            ("OMAKURE_NODE_TEST_MODE", "1"),
            ("OMAKURE_NODE_STATE_DIR", state_env.as_str()),
            ("OMAKURE_NODE_CONFIG", config_env.as_str()),
        ],
        std::time::Duration::from_secs(10),
    );
    BehavioralContext {
        workspace,
        repository,
        server,
        fixture,
    }
}

fn identity_material(ctx: &BehavioralContext) -> Result<(String, String, String), String> {
    let context = node_context(ctx);
    let identity =
        NodeIdentity::load_existing(&context).map_err(|e| format!("load identity: {e}"))?;
    let transport = LocalTransport::load_existing(&context, &identity)
        .map_err(|e| format!("load transport: {e}"))?;
    Ok((
        identity.public_status().node_id.clone(),
        identity.public_status().public_key_hex.clone(),
        enrollment::hex_bytes(transport.certificate().as_bytes()),
    ))
}

fn configure_direct(
    ctx: &BehavioralContext,
    port: u16,
    peers: &[(&str, u16)],
    organization: &str,
    push: Option<(&str, &str)>,
) -> Result<(), String> {
    let peer_list = peers
        .iter()
        .map(|(id, p)| format!("\"{id}@127.0.0.1:{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut replacements = vec![
        (
            "static_peers = []",
            format!("direct_bind = \"127.0.0.1:{port}\"\nstatic_peers = [{peer_list}]"),
        ),
        ("id = \"\"", format!("id = \"{organization}\"")),
    ];
    if let Some((key_id, public_key)) = push {
        replacements.push((
            "enrollment = \"disabled\"",
            "enrollment = \"manual\"".into(),
        ));
        replacements.push((
            "allow_baseline_push = false",
            "allow_baseline_push = true".into(),
        ));
        replacements.push(("baseline_publishers = []", format!("baseline_publishers = [{{ key_id = \"{key_id}\", public_key = \"{public_key}\", revoked = false }}]")));
    }
    edit_config(ctx, &replacements)
}

fn trust_with_material(
    ctx: &BehavioralContext,
    node_id: &str,
    public_key: &str,
    certificate: &str,
    role: &str,
    capabilities: &[&str],
) -> Result<(), String> {
    let actor = ctx.authorized_actor();
    let mut args = vec![
        "trust",
        "--node-id",
        node_id,
        "--public-key",
        public_key,
        "--transport-certificate",
        certificate,
        "--role",
        role,
        "--actor",
        actor,
        "--reason",
        "baseline-fleet",
        "--confirmed",
    ];
    for capability in capabilities {
        args.extend(["--capability", capability]);
    }
    require_ok(&node_cli_any(ctx, &args), "baseline fleet trust")
}

fn await_session(ctx: &BehavioralContext, peer_count: u64) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let response = ctx.server.get("/v1/node/status");
        if response.status == 200 {
            let transport = response.json()["data"]["transport"].clone();
            if transport["expected_peer_count"].as_u64() == Some(peer_count)
                && transport["expected_connected_peer_count"].as_u64() == Some(peer_count)
            {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("baseline fleet did not establish its direct session".into())
}
fn observed_peer_state(ctx: &BehavioralContext, node_id: &str) -> Result<String, String> {
    let value = node_cli_any(ctx, &["peers"]);
    require_payload(&value, "receiver peer state")?;
    value["data"]
        .as_array()
        .and_then(|peers| peers.iter().find(|peer| peer["node_id"] == node_id))
        .and_then(|peer| peer["state"].as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("receiver did not report peer {node_id}: {value}"))
}
fn rollback_expectation(ctx: &BehavioralContext) -> Result<(String, Vec<Vec<u8>>), String> {
    let retained = fs::read(ctx.workspace.path().join(".omakure/baseline-previous.json"))
        .map_err(|error| format!("read retained baseline: {error}"))?;
    let retained: RetainedBaseline = serde_json::from_slice(&retained)
        .map_err(|error| format!("parse retained baseline: {error}"))?;
    let push = BaselinePush::parse(&retained.push)
        .map_err(|error| format!("parse retained baseline push: {error:?}"))?;
    let manifest = SignedBaselineManifest::decode(&push.manifest)
        .map_err(|error| format!("decode retained baseline manifest: {error}"))?;
    let baseline_id = manifest
        .baseline_id()
        .map_err(|error| format!("derive retained baseline id: {error}"))?;
    Ok((enrollment::hex_bytes(&baseline_id), push.bodies))
}

fn assert_rollback_files(
    ctx: &BehavioralContext,
    expected_bodies: &[Vec<u8>],
) -> Result<(), String> {
    for (name, expected) in ["base-a.sh", "base-b.sh"].into_iter().zip(expected_bodies) {
        let actual = fs::read(ctx.workspace.path().join(name))
            .map_err(|error| format!("read restored {name}: {error}"))?;
        if &actual != expected {
            return Err(format!("rollback did not restore {name}"));
        }
    }
    Ok(())
}
fn mutate_rollback_files(
    ctx: &BehavioralContext,
    expected_bodies: &[Vec<u8>],
) -> Result<(), String> {
    for (name, expected) in ["base-a.sh", "base-b.sh"].into_iter().zip(expected_bodies) {
        let mut mutated = expected.clone();
        mutated.extend_from_slice(b"\n# current baseline mutation\n");
        fs::write(ctx.workspace.path().join(name), mutated)
            .map_err(|error| format!("mutate current {name}: {error}"))?;
        let current = fs::read(ctx.workspace.path().join(name))
            .map_err(|error| format!("read mutated {name}: {error}"))?;
        if current == *expected {
            return Err(format!("rollback fixture mutation was a no-op for {name}"));
        }
    }
    Ok(())
}

fn node_baseline_push(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli0, http0) = pair(parent, "node_baseline_push");
    let peer0 = parent.derive_node("node_baseline_push_peer", CAPS);
    edit_config(&peer0, &[("id = \"\"", "id = \"parity-org\"".into())])?;
    initialize(&peer0)?;
    let (peer_manifest, publisher_id, publisher_key) = baseline_fixture(&peer0)?;
    let manifest_path = cli0.workspace.path().join("base.omb");
    fs::copy(&peer_manifest, &manifest_path).map_err(|e| format!("copy baseline manifest: {e}"))?;
    for name in ["base-a.sh", "base-b.sh"] {
        fs::copy(
            peer0.workspace.path().join(name),
            cli0.workspace.path().join(name),
        )
        .map_err(|e| format!("copy baseline script: {e}"))?;
    }
    let manifest = manifest_path.to_string_lossy().to_string();
    initialize(&http0)?;
    let cli_material = identity_material(&cli0)?;
    let http_material = identity_material(&http0)?;
    let peer_material = identity_material(&peer0)?;
    let cli_port = super::support::unique_loopback_port();
    let http_port = super::support::unique_loopback_port();
    let peer_port = super::support::unique_loopback_port();
    configure_direct(
        &cli0,
        cli_port,
        &[(&peer_material.0, peer_port)],
        "parity-org",
        None,
    )?;
    configure_direct(
        &http0,
        http_port,
        &[(&peer_material.0, peer_port)],
        "parity-org",
        None,
    )?;
    configure_direct(
        &peer0,
        peer_port,
        &[(&cli_material.0, cli_port), (&http_material.0, http_port)],
        "parity-org",
        Some((&publisher_id, &publisher_key)),
    )?;
    trust_with_material(
        &cli0,
        &peer_material.0,
        &peer_material.1,
        &peer_material.2,
        "performer",
        &["baseline-push"],
    )?;
    trust_with_material(
        &http0,
        &peer_material.0,
        &peer_material.1,
        &peer_material.2,
        "performer",
        &["baseline-push"],
    )?;
    trust_with_material(
        &peer0,
        &cli_material.0,
        &cli_material.1,
        &cli_material.2,
        "conductor",
        &["baseline-push"],
    )?;
    trust_with_material(
        &peer0,
        &http_material.0,
        &http_material.1,
        &http_material.2,
        "conductor",
        &["baseline-push"],
    )?;
    let cli = restart_node(cli0);
    let http_ctx = restart_node(http0);
    let peer = restart_node(peer0);
    await_session(&cli, 1)?;
    await_session(&http_ctx, 1)?;
    let c = node_cli_any(
        &cli,
        &[
            "baseline",
            "push",
            "--peer-node-id",
            &peer_material.0,
            "--manifest",
            &manifest,
        ],
    );
    require_ok(&c, "CLI baseline push")?;
    if c["data"]["accepted"] != true {
        return Err(format!("CLI baseline push was not accepted: {c}"));
    }
    let cli_peer_state = observed_peer_state(&peer, &cli_material.0)?;
    if cli_peer_state != "active" {
        return Err(format!(
            "receiver did not retain active CLI peer after baseline push: {cli_peer_state}"
        ));
    }
    let manifest_hex = fs::read(&manifest)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let scripts = ["base-a.sh", "base-b.sh"]
        .iter()
        .map(|name| {
            fs::read(cli.workspace.path().join(name))
                .map(|bytes| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let body = json!({"peer_node_id": peer_material.0, "manifest": manifest_hex, "scripts": scripts, "wait_seconds": 2});
    assert_post_auth(&http_ctx, "/v1/node/baselines", &body)?;
    let (status, h) = http(
        &http_ctx,
        http_ctx.server.post_json("/v1/node/baselines", &body),
    )?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP baseline push")?;
    if h["data"]["accepted"] != true {
        return Err(format!("HTTP baseline push was not accepted: {h}"));
    }
    let http_peer_state = observed_peer_state(&peer, &http_material.0)?;
    if http_peer_state != "active" {
        return Err(format!(
            "receiver did not retain active HTTP peer after baseline push: {http_peer_state}"
        ));
    }
    evidence(
        projected(
            &c,
            json!({"accepted": c["data"]["accepted"], "baseline_id": c["data"]["baseline_id"], "peer_state": cli_peer_state}),
        ),
        (
            status,
            projected(
                &h,
                json!({"accepted": h["data"]["accepted"], "baseline_id": h["data"]["baseline_id"], "peer_state": http_peer_state}),
            ),
        ),
    )
}

fn node_baseline_rollback(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli0, http0) = pair(parent, "node_baseline_rollback");
    seed_rollback_baseline(&cli0)?;
    seed_rollback_baseline(&http0)?;
    let (expected_cli_id, expected_cli_bodies) = rollback_expectation(&cli0)?;
    let (expected_http_id, expected_http_bodies) = rollback_expectation(&http0)?;
    mutate_rollback_files(&cli0, &expected_cli_bodies)?;
    mutate_rollback_files(&http0, &expected_http_bodies)?;
    let cli = restart_node(cli0);
    let http_ctx = restart_node(http0);
    let c = node_cli_any(&cli, ["baseline", "rollback", "--confirmed"].as_slice());
    require_ok(&c, "CLI baseline rollback")?;
    if c["data"]["baseline_id"] != expected_cli_id {
        return Err(format!("CLI rollback selected the wrong baseline: {c}"));
    }
    assert_rollback_files(&cli, &expected_cli_bodies)?;
    let body = json!({"confirmed":true});
    assert_post_auth(&http_ctx, "/v1/node/baseline/rollback", &body)?;
    let (status, h) = http(
        &http_ctx,
        http_ctx
            .server
            .post_json("/v1/node/baseline/rollback", &body),
    )?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP baseline rollback")?;
    if h["data"]["baseline_id"] != expected_http_id {
        return Err(format!("HTTP rollback selected the wrong baseline: {h}"));
    }
    assert_rollback_files(&http_ctx, &expected_http_bodies)?;
    evidence(
        json!({
            "ok": c["ok"],
            "result_kind": c["data"]["baseline_id"],
            "data": {
                "baseline_id": c["data"]["baseline_id"],
                "accepted": c["data"]["baseline_id"].as_str().is_some_and(|id| !id.is_empty()),
                "content_match": true,
                "files_restored": true
            }
        }),
        (
            status,
            json!({
                "ok": h["ok"],
                "result_kind": h["data"]["baseline_id"],
                "data": {
                    "baseline_id": h["data"]["baseline_id"],
                    "accepted": h["data"]["baseline_id"].as_str().is_some_and(|id| !id.is_empty()),
                    "content_match": true,
                    "files_restored": true
                }
            }),
        ),
    )
}

fn node_enroll_approve(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_enroll_approve");
    initialize(&cli)?;
    initialize(&http_ctx)?;
    enable_manual(&cli)?;
    enable_manual(&http_ctx)?;
    let cli_candidate = parent.derive_node("node_enroll_approve_cli_candidate", CAPS);
    let http_candidate = parent.derive_node("node_enroll_approve_http_candidate", CAPS);
    let (cli_id, cli_req, cli_cert, cli_code) = enrollment_material(&cli, &cli_candidate)?;
    let (http_id, http_req, http_cert, http_code) =
        enrollment_material(&http_ctx, &http_candidate)?;
    let staged = stage(&cli, &cli_req, &cli_cert)?;
    if staged["state"] != "pending" || staged["node_id"] != cli_id {
        return Err(format!(
            "CLI enrollment stage did not create the generated pending peer: {staged}"
        ));
    }
    let actor = cli.authorized_actor();
    let c = node_cli_any(
        &cli,
        &[
            "enroll",
            "approve",
            "--request",
            &cli_req,
            "--transport-certificate",
            &cli_cert,
            "--code",
            &cli_code,
            "--actor",
            actor,
            "--reason",
            "deterministic-approve",
            "--confirmed",
        ],
    );
    require_ok(&c, "CLI enrollment approve")?;
    if c["data"]["state"] != "active" {
        return Err(format!("CLI approval did not activate peer: {c}"));
    }
    let stage_body = json!({"request_hex":http_req,"transport_certificate":http_cert});
    let (stage_status, stage_body_json) = http(
        &http_ctx,
        http_ctx
            .server
            .post_json("/v1/node/enrollments", &stage_body),
    )?;
    expect_status(stage_status, 200, &stage_body_json)?;
    if stage_body_json["data"]["state"] != "pending"
        || stage_body_json["data"]["node_id"] != http_id
    {
        return Err(format!(
            "HTTP enrollment stage did not create the generated pending peer: {stage_body_json}"
        ));
    }
    let body = json!({"request_hex":http_req,"transport_certificate":http_cert,"code":http_code,"actor":actor,"reason":"deterministic-approve","confirmed":true});
    let path = format!("/v1/node/enrollments/{http_id}/approve");
    assert_post_auth(&http_ctx, &path, &body)?;
    let (status, h) = http(&http_ctx, http_ctx.server.post_json(&path, &body))?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP enrollment approve")?;
    if h["data"]["state"] != "active" {
        return Err(format!("HTTP approval did not activate peer: {h}"));
    }
    evidence(
        projected(&c, stable_peer(&c)),
        (status, projected(&h, stable_peer(&h))),
    )
}
fn node_enroll_reject(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_enroll_reject");
    initialize(&cli)?;
    initialize(&http_ctx)?;
    enable_manual(&cli)?;
    enable_manual(&http_ctx)?;
    let cli_candidate = parent.derive_node("node_enroll_reject_cli_candidate", CAPS);
    let http_candidate = parent.derive_node("node_enroll_reject_http_candidate", CAPS);
    let (cli_id, cli_req, cli_cert, _) = enrollment_material(&cli, &cli_candidate)?;
    let (http_id, http_req, http_cert, _) = enrollment_material(&http_ctx, &http_candidate)?;
    stage(&cli, &cli_req, &cli_cert)?;
    let actor = cli.authorized_actor();
    let c = node_cli_any(
        &cli,
        &[
            "enroll",
            "reject",
            &cli_id,
            "--actor",
            actor,
            "--reason",
            "deterministic-reject",
            "--confirmed",
        ],
    );
    require_ok(&c, "CLI enrollment reject")?;
    if c["data"]["state"] != "suspended" {
        return Err(format!("CLI rejection did not suspend peer: {c}"));
    }
    let stage_body = json!({"request_hex":http_req,"transport_certificate":http_cert});
    let (stage_status, stage_json) = http(
        &http_ctx,
        http_ctx
            .server
            .post_json("/v1/node/enrollments", &stage_body),
    )?;
    expect_status(stage_status, 200, &stage_json)?;
    let body = json!({"actor":actor,"reason":"deterministic-reject","confirmed":true});
    let path = format!("/v1/node/enrollments/{http_id}/reject");
    assert_post_auth(&http_ctx, &path, &body)?;
    let (status, h) = http(&http_ctx, http_ctx.server.post_json(&path, &body))?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP enrollment reject")?;
    if h["data"]["state"] != "suspended" {
        return Err(format!("HTTP rejection did not suspend peer: {h}"));
    }
    evidence(
        projected(&c, stable_peer(&c)),
        (status, projected(&h, stable_peer(&h))),
    )
}

fn node_discovery(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    if omakure::discovery::platform_supported() {
        node_discovery_supported(parent)
    } else {
        node_discovery_unsupported_platform(parent)
    }
}

fn node_discovery_supported(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_discovery");
    initialize(&cli)?;
    initialize(&http_ctx)?;
    edit_config(&cli, &[("enabled = false", "enabled = true".into())])?;
    edit_config(&http_ctx, &[("enabled = false", "enabled = true".into())])?;
    // Scan before starting the HTTP service so both probes can use the frozen
    // discovery port without colliding.
    let c = node_cli_any(&cli, &["discovery", "--wait-seconds", "1"]);
    require_ok(&c, "CLI discovery")?;
    let c_data = &c["data"];
    if c_data["enabled"] != true
        || c_data["listening"] != true
        || c_data["candidate_count"] != 0
        || c_data["accepted_datagrams"] != 0
    {
        return Err(format!(
            "CLI discovery did not perform an isolated fresh scan: {c}"
        ));
    }
    let http_ctx = restart_node(http_ctx);
    let candidate = BehavioralContext::new_node("node_discovery_candidate", CAPS);
    initialize(&candidate)?;
    let candidate_identity = NodeIdentity::load_existing(&node_context(&candidate))
        .map_err(|error| format!("load discovery candidate identity: {error}"))?;
    let sender = UdpSocket::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    let beacon = Beacon::create(
        &candidate_identity,
        4242,
        [7; 16],
        1,
        candidate.fresh_clock_seconds(),
        None,
    )
    .map_err(|error| format!("create discovery beacon: {error}"))?
    .encode()
    .map_err(|error| format!("encode discovery beacon: {error}"))?;
    sender
        .send_to(&beacon, "127.0.0.1:38383")
        .map_err(|error| format!("send discovery beacon: {error}"))?;
    assert_get_auth(&http_ctx, "/v1/node/discovery?include_addresses=true")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let (status, h) = loop {
        let response = http(
            &http_ctx,
            http_ctx
                .server
                .get("/v1/node/discovery?include_addresses=true"),
        )?;
        if response.1["data"]["accepted_datagrams"]
            .as_u64()
            .unwrap_or(0)
            >= 1
            || std::time::Instant::now() >= deadline
        {
            break response;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP discovery")?;
    let h_data = &h["data"];
    if h_data["enabled"] != true
        || h_data["listening"] != true
        || h_data["accepted_datagrams"].as_u64().unwrap_or(0) < 1
        || h_data["candidate_count"].as_u64().unwrap_or(0) < 1
    {
        return Err(format!(
            "HTTP discovery did not expose its live in-memory snapshot: {h}"
        ));
    }
    let cli_data = json!({"enabled": c_data["enabled"], "listening": c_data["listening"], "candidate_count": c_data["candidate_count"], "accepted_datagrams": c_data["accepted_datagrams"]});
    let http_data = json!({"enabled": h_data["enabled"], "listening": h_data["listening"], "candidate_count": h_data["candidate_count"], "accepted_datagrams": h_data["accepted_datagrams"]});
    let mut result = evidence(
        with_http_status(&projected(&c, cli_data), status),
        (status, with_http_status(&projected(&h, http_data), status)),
    )?;
    result.semantic_difference = Some("discovery-snapshot".into());
    Ok(result)
}

fn node_discovery_unsupported_platform(
    parent: &BehavioralContext,
) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_discovery");
    initialize(&cli)?;
    initialize(&http_ctx)?;
    edit_config(&cli, &[("enabled = false", "enabled = true".into())])?;
    edit_config(&http_ctx, &[("enabled = false", "enabled = true".into())])?;
    let c = node_cli_any(&cli, &["discovery", "--wait-seconds", "1"]);
    if c["ok"] != false || c["error"]["code"] != "discovery_unsupported_platform" {
        return Err(format!(
            "CLI discovery must return discovery_unsupported_platform on unsupported platforms: {c}"
        ));
    }
    let http_ctx = restart_node(http_ctx);
    assert_get_auth(&http_ctx, "/v1/node/discovery?include_addresses=true")?;
    let (status, h) = http(
        &http_ctx,
        http_ctx
            .server
            .get("/v1/node/discovery?include_addresses=true"),
    )?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP discovery")?;
    let h_data = &h["data"];
    if h_data["supported"] != false
        || h_data["enabled"] != true
        || h_data["listening"] != false
        || h_data["candidate_count"] != 0
        || h_data["accepted_datagrams"] != 0
    {
        return Err(format!(
            "HTTP discovery did not expose disabled unsupported-platform status: {h}"
        ));
    }
    let cli_data = json!({
        "enabled": true,
        "listening": false,
        "candidate_count": 0,
        "accepted_datagrams": 0,
    });
    let http_data = json!({
        "enabled": h_data["enabled"],
        "listening": h_data["listening"],
        "candidate_count": h_data["candidate_count"],
        "accepted_datagrams": h_data["accepted_datagrams"],
    });
    let mut result = evidence(
        with_http_status(&projected(&c, cli_data), status),
        (status, with_http_status(&projected(&h, http_data), status)),
    )?;
    result.semantic_difference = Some("discovery-snapshot".into());
    Ok(result)
}
fn node_cue(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_cue");
    initialize(&cli)?;
    initialize(&http_ctx)?;
    let direct_port = super::support::unique_loopback_port();
    edit_config(
        &http_ctx,
        &[(
            "static_peers = []",
            format!("direct_bind = \"127.0.0.1:{direct_port}\"\nstatic_peers = []"),
        )],
    )?;
    let http_ctx = restart_node(http_ctx);
    let tail = [
        "cue",
        "--endpoint",
        "127.0.0.1:9",
        "--peer-node-id",
        PEER_ID,
        "--script",
        "fixture.sh",
        "--reason",
        "deterministic-cue",
        "--wait-seconds",
        "0",
        "--direct",
    ];
    let c = node_cli_any(&cli, &tail);
    if c["ok"] == true || c["error"]["code"] != "transport_internal" || !c["data"].is_null() {
        return Err(format!(
            "CLI cue did not report the refused direct dial: {c}"
        ));
    }
    let body = json!({"peer_node_id":PEER_ID,"script":"fixture.sh","reason":"deterministic-cue","wait_seconds":0});
    assert_post_auth(&http_ctx, "/v1/node/cues", &body)?;
    let (status, h) = http(&http_ctx, http_ctx.server.post_json("/v1/node/cues", &body))?;
    expect_status(status, 404, &h)?;
    if h["error"]["code"] != "not_found" {
        return Err(format!("HTTP cue did not report missing session: {h}"));
    }
    if !h["data"].is_null() {
        return Err(format!(
            "HTTP cue unexpectedly returned an accepted payload: {h}"
        ));
    }
    let mut result = evidence(
        json!({"ok": c["ok"], "error": {"code": c["error"]["code"]}, "http_status": status}),
        (
            status,
            json!({"ok": h["ok"], "error": {"code": h["error"]["code"]}, "http_status": status}),
        ),
    )?;
    result.semantic_difference = Some("cue-session".into());
    Ok(result)
}
fn node_enroll_request(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_enroll_request");
    initialize(&cli)?;
    initialize(&http_ctx)?;
    enable_manual(&cli)?;
    enable_manual(&http_ctx)?;
    let cli_id = identity_material(&cli)?.0;
    let conductor_port = super::support::unique_loopback_port();
    configure_direct(&http_ctx, conductor_port, &[], "parity-org", None)?;
    let http_ctx = restart_node(http_ctx);
    let candidate = parent.derive_node("node_enroll_request_candidate", CAPS);
    let (http_id, request, certificate, _) = enrollment_material(&http_ctx, &candidate)?;
    let endpoint = format!("127.0.0.1:{conductor_port}");
    let c = node_cli_any(
        &cli,
        &[
            "enroll",
            "request",
            "--endpoint",
            &endpoint,
            "--role",
            "performer",
            "--capability",
            "remote-run",
            "--lifetime-seconds",
            "60",
        ],
    );
    require_ok(&c, "CLI enrollment request")?;
    if c["data"]["state"] != "pending" {
        return Err(format!("CLI enrollment request was not pending: {c}"));
    }
    let generated_request_hex = c["data"]["request_hex"]
        .as_str()
        .ok_or("CLI enrollment request did not return request bytes")?;
    let generated_request_bytes =
        enrollment::parse_hex(generated_request_hex, generated_request_hex.len() / 2)
            .map_err(|error| format!("decode CLI enrollment request: {error}"))?;
    let generated_request = ManualEnrollmentRequest::decode(&generated_request_bytes)
        .map_err(|error| format!("parse CLI enrollment request: {error}"))?;
    if generated_request.proposer_node_id != cli_id {
        return Err(format!(
            "CLI enrollment request identity mismatch: expected {cli_id}, got {}",
            generated_request.proposer_node_id
        ));
    }
    let staged = http_ctx.server.get("/v1/node/enrollments");
    expect_status(staged.status, 200, &staged.json())?;
    let staged_peers = staged.json();
    if !staged_peers["data"]
        .as_array()
        .is_some_and(|peers| peers.iter().any(|peer| peer["node_id"] == cli_id))
    {
        return Err(format!(
            "conductor did not stage the generated CLI enrollment request: {staged_peers}"
        ));
    }
    let body = json!({"request_hex":request,"transport_certificate":certificate});
    assert_post_auth(&http_ctx, "/v1/node/enrollments", &body)?;
    let (status, h) = http(
        &http_ctx,
        http_ctx.server.post_json("/v1/node/enrollments", &body),
    )?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP enrollment stage")?;
    if h["data"]["state"] != "pending" || h["data"]["node_id"] != http_id {
        return Err(format!(
            "HTTP enrollment did not stage the generated pending peer: {h}"
        ));
    }
    let http_projection = projected(
        &h,
        json!({"state": h["data"]["state"], "source": h["data"]["source"], "node_id": h["data"]["node_id"]}),
    );
    let cli_projection = json!({"ok": c["ok"], "data": {"state": c["data"]["state"], "source": "direct", "node_id": cli_id}, "http_status": status});
    let mut result = evidence(
        cli_projection,
        (status, with_http_status(&http_projection, status)),
    )?;
    result.semantic_difference = Some("enroll-stage-dial".into());
    Ok(result)
}

fn node_enroll_apply(parent: &BehavioralContext) -> Result<ProbeEvidence, String> {
    let (cli, http_ctx) = pair(parent, "node_enroll_apply");
    initialize(&cli)?;
    initialize(&http_ctx)?;
    let issuer = parent.derive_node("node_enroll_apply_issuer", CAPS);
    initialize(&issuer)?;
    edit_config(&issuer, &[("id = \"\"", "id = \"parity-org\"".into())])?;
    let authority = node_cli_any(&issuer, &["authority", "create", "--confirmed"]);
    require_ok(&authority, "authority create")?;
    let cli_target_id = node_cli_any(&cli, &["status"])["data"]["identity"]["node_id"]
        .as_str()
        .ok_or("missing CLI target node id")?
        .to_string();
    let http_target_id = node_cli_any(&http_ctx, &["status"])["data"]["identity"]["node_id"]
        .as_str()
        .ok_or("missing HTTP target node id")?
        .to_string();
    let authority_key_id = authority["data"]["key_id"]
        .as_str()
        .ok_or("missing authority key id")?;
    let authority_key = authority["data"]["public_key"]
        .as_str()
        .ok_or("missing authority public key")?;
    let cli_bootstrap_token = b"behavioral-parity-cli-bootstrap-token-000000";
    let http_bootstrap_token = b"behavioral-parity-http-bootstrap-token-000000";
    let cli_token_hash =
        enrollment::hex_bytes(&enrollment::hash_bootstrap_token(cli_bootstrap_token));
    let http_token_hash =
        enrollment::hex_bytes(&enrollment::hash_bootstrap_token(http_bootstrap_token));
    let nonce = "00112233445566778899aabbccddeeff";
    let nonce_bytes = enrollment::parse_hex(nonce, 16).map_err(|e| format!("nonce: {e}"))?;
    let nonce_hash = enrollment::hex_bytes(&enrollment::hash_bootstrap_nonce(&nonce_bytes));
    for (target, token_hash) in [
        (&cli, cli_token_hash.as_str()),
        (&http_ctx, http_token_hash.as_str()),
    ] {
        edit_config(
            target,
            &[
                (
                    "enrollment = \"disabled\"",
                    "enrollment = \"signed-bundle\"".into(),
                ),
                (
                    "authorities = []",
                    format!(
                        "authorities = [{{ key_id = \"{authority_key_id}\", public_key = \"{authority_key}\", revoked = false }}]"
                    ),
                ),
                (
                    "bootstrap_token_hash = \"\"",
                    format!("bootstrap_token_hash = \"{token_hash}\""),
                ),
                (
                    "bootstrap_nonce_hash = \"\"",
                    format!("bootstrap_nonce_hash = \"{nonce_hash}\""),
                ),
                ("id = \"\"", "id = \"parity-org\"".into()),
            ],
        )?;
    }
    let issue = |audience: &str| -> Result<String, String> {
        let issued = node_cli_any(
            &issuer,
            &[
                "authority",
                "issue",
                "--audience",
                audience,
                "--role",
                "conductor",
                "--capability",
                "remote-run",
                "--lifetime-seconds",
                "3600",
            ],
        );
        require_ok(&issued, "authority issue")?;
        Ok(issued["data"]["bundle_hex"]
            .as_str()
            .ok_or("missing bundle".to_string())?
            .to_string())
    };
    let cli_bundle = issue(&cli_target_id)?;
    let http_bundle = issue(&http_target_id)?;
    let bundle_path = cli.workspace.path().join("bundle.hex");
    let wrong_bundle_path = cli.workspace.path().join("wrong-target-bundle.hex");
    let token_path = cli.workspace.path().join("bootstrap.token");
    fs::write(&bundle_path, &cli_bundle).map_err(|e| e.to_string())?;
    fs::write(&wrong_bundle_path, &http_bundle).map_err(|e| e.to_string())?;
    fs::write(&token_path, cli_bootstrap_token).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    let http_token_path = http_ctx.workspace.path().join("bootstrap.token");
    fs::write(&http_token_path, http_bootstrap_token).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&http_token_path, fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    let http_token_env = http_token_path.to_string_lossy().to_string();
    let http_ctx = restart_node_with_env(
        http_ctx,
        &[("OMAKURE_BOOTSTRAP_TOKEN_FILE", http_token_env.as_str())],
    );
    let nonce_arg = nonce.to_string();
    let bundle_arg = bundle_path.to_string_lossy().to_string();
    let wrong_bundle_arg = wrong_bundle_path.to_string_lossy().to_string();
    let token_arg = token_path.to_string_lossy().to_string();
    let cli_target_verified = verify_bundle_target(&cli_bundle, &cli_target_id)?;
    let http_target_verified = verify_bundle_target(&http_bundle, &http_target_id)?;

    let cli_peers_before = node_cli_any(&cli, &["peers"]);
    require_payload(&cli_peers_before, "CLI peers before wrong-target apply")?;
    let (http_peers_before_status, http_peers_before) =
        http(&http_ctx, http_ctx.server.get("/v1/node/peers"))?;
    expect_status(http_peers_before_status, 200, &http_peers_before)?;
    require_payload(&http_peers_before, "HTTP peers before wrong-target apply")?;

    let cli_wrong = node_cli_any(
        &cli,
        &[
            "enroll",
            "apply",
            "--bundle-file",
            &wrong_bundle_arg,
            "--bootstrap-token-file",
            &token_arg,
            "--bootstrap-nonce",
            &nonce_arg,
        ],
    );
    require_audience_mismatch(&cli_wrong, "CLI wrong-target bundle apply")?;
    let cli_wrong_target_rejected = cli_wrong["ok"] == false
        && cli_wrong["error"]["code"] == "enrollment_mismatch"
        && cli_wrong["error"]["message"]
            == "signed enrollment bundle audience does not match this node";
    require_token_untouched(
        &token_path,
        cli_bootstrap_token,
        "CLI wrong-target bundle apply",
    )?;
    let cli_peers_after_wrong = node_cli_any(&cli, &["peers"]);
    require_payload(&cli_peers_after_wrong, "CLI peers after wrong-target apply")?;
    if cli_peers_after_wrong["data"] != cli_peers_before["data"] {
        return Err(format!(
            "CLI wrong-target apply changed trusted-peer state: before={}, after={}",
            cli_peers_before["data"], cli_peers_after_wrong["data"]
        ));
    }

    let http_wrong_body = json!({"bundle_hex": cli_bundle, "bootstrap_nonce": nonce});
    assert_post_auth(&http_ctx, "/v1/node/enrollment/bundle", &http_wrong_body)?;
    let (http_wrong_status, http_wrong) = http(
        &http_ctx,
        http_ctx
            .server
            .post_json("/v1/node/enrollment/bundle", &http_wrong_body),
    )?;
    expect_status(http_wrong_status, 409, &http_wrong)?;
    require_audience_mismatch(&http_wrong, "HTTP wrong-target bundle apply")?;
    let http_wrong_target_rejected = http_wrong_status == 409
        && http_wrong["ok"] == false
        && http_wrong["error"]["code"] == "enrollment_mismatch"
        && http_wrong["error"]["message"]
            == "signed enrollment bundle audience does not match this node";
    require_token_untouched(
        &http_token_path,
        http_bootstrap_token,
        "HTTP wrong-target bundle apply",
    )?;
    let (http_peers_after_wrong_status, http_peers_after_wrong) =
        http(&http_ctx, http_ctx.server.get("/v1/node/peers"))?;
    expect_status(http_peers_after_wrong_status, 200, &http_peers_after_wrong)?;
    require_payload(
        &http_peers_after_wrong,
        "HTTP peers after wrong-target apply",
    )?;
    if http_peers_after_wrong["data"] != http_peers_before["data"] {
        return Err(format!(
            "HTTP wrong-target apply changed trusted-peer state: before={}, after={}",
            http_peers_before["data"], http_peers_after_wrong["data"]
        ));
    }

    let c = node_cli_any(
        &cli,
        &[
            "enroll",
            "apply",
            "--bundle-file",
            &bundle_arg,
            "--bootstrap-token-file",
            &token_arg,
            "--bootstrap-nonce",
            &nonce_arg,
        ],
    );
    require_ok(&c, "CLI bundle apply")?;
    let cli_correct_target_succeeded = c["ok"] == true && c["data"]["state"] == "active";
    if !cli_correct_target_succeeded {
        return Err(format!("CLI bundle apply did not activate peer: {c}"));
    }
    let http_body = json!({"bundle_hex": http_bundle, "bootstrap_nonce": nonce});
    assert_post_auth(&http_ctx, "/v1/node/enrollment/bundle", &http_body)?;
    let (status, h) = http(
        &http_ctx,
        http_ctx
            .server
            .post_json("/v1/node/enrollment/bundle", &http_body),
    )?;
    expect_status(status, 200, &h)?;
    require_ok(&h, "HTTP bundle apply")?;
    let http_correct_target_succeeded =
        status == 200 && h["ok"] == true && h["data"]["state"] == "active";
    if !http_correct_target_succeeded {
        return Err(format!("HTTP bundle apply did not activate peer: {h}"));
    }
    let target_binding_verified = cli_target_verified
        && http_target_verified
        && cli_wrong_target_rejected
        && http_wrong_target_rejected
        && cli_correct_target_succeeded
        && http_correct_target_succeeded;
    if !target_binding_verified {
        return Err(format!(
            "target binding probe did not observe both mismatch rejections and correct-target success: cli_wrong={cli_wrong}, http_wrong_status={http_wrong_status}, http_wrong={http_wrong}, cli_correct={c}, http_correct={h}"
        ));
    }
    let mut cli_peer = stable_peer(&c);
    cli_peer["target_binding_verified"] = json!(target_binding_verified);
    let mut http_peer = stable_peer(&h);
    http_peer["target_binding_verified"] = json!(target_binding_verified);
    let mut result = evidence(
        with_http_status(&projected(&c, cli_peer), status),
        (status, with_http_status(&projected(&h, http_peer), status)),
    )?;
    result.semantic_difference = Some("enroll-token-source".into());
    Ok(result)
}
