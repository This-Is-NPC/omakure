//! One signed baseline, end to end, between two real nodes.
//!
//! The gates, the manifest format, the install, and the rollback are all proven
//! in isolation elsewhere. What none of that proves is that a Conductor is
//! *told* what became of a push: the answer travels back as a `baseline_ack` on
//! the same standing session the push went out on, and an operator has nothing
//! else to read. A push that installs but reports `answered: false` is
//! indistinguishable from one refused on trust, role, or capability.
//!
//! Two real `node serve` processes, real transport, no mocks.

mod support;

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const TOKEN: &str = "baseline-push-e2e-token-with-enough-entropy-01";
const ORGANIZATION: &str = "baseline-e2e-fleet";

fn run_node(workspace: &Path, args: &[String]) -> Output {
    let output = Command::new(support::omakure_bin())
        .arg("--scripts-dir")
        .arg(workspace)
        .arg("--json")
        .arg("node")
        .arg("--node-state-dir")
        .arg(workspace.join(".node-state"))
        .arg("--node-config")
        .arg(workspace.join("node.toml"))
        .args(args)
        .env("OMAKURE_NODE_TEST_MODE", "1")
        .env("OMAKURE_API_TOKEN", TOKEN)
        .output()
        .expect("run node command");
    assert!(
        output.status.code().is_some(),
        "node {args:?} was killed by a signal"
    );
    output
}

fn assert_success_named(label: &str, output: &Output) -> Value {
    assert!(
        output.status.success(),
        "node {label} failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = support::json_envelope(&output.stdout);
    assert_eq!(envelope["ok"], true, "node {label} envelope: {envelope}");
    envelope["data"].clone()
}

fn init_node(workspace: &Path) -> Value {
    assert_success_named("init", &run_node(workspace, &["init".to_string()]));
    assert_success_named("status", &run_node(workspace, &["status".to_string()]))
}

fn serve(workspace: &Path) -> support::HttpServer {
    support::HttpServer::start_node_service(
        workspace,
        TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "node:read",
            "--capability",
            "node:write",
        ],
        &[],
        Duration::from_secs(20),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn trust_peer(
    workspace: &Path,
    peer_workspace: &Path,
    peer_status: &Value,
    role: &str,
    capabilities: &[&str],
) {
    let certificate = hex(
        &std::fs::read(peer_workspace.join(".node-state/transport.cert"))
            .expect("read peer transport certificate"),
    );
    let mut args = vec![
        "trust".to_string(),
        "--node-id".to_string(),
        peer_status["identity"]["node_id"].as_str().unwrap().into(),
        "--public-key".to_string(),
        peer_status["identity"]["public_key"]
            .as_str()
            .unwrap()
            .into(),
        "--transport-certificate".to_string(),
        certificate,
        "--role".to_string(),
        role.to_string(),
        "--actor".to_string(),
        "baseline-push-e2e".to_string(),
        "--reason".to_string(),
        "baseline delivery certification".to_string(),
        "--confirmed".to_string(),
    ];
    for capability in capabilities {
        args.push("--capability".to_string());
        args.push((*capability).to_string());
    }
    assert_eq!(
        assert_success_named("trust", &run_node(workspace, &args))["state"],
        "active"
    );
}

/// A baseline member. Real content, so the manifest hashes something specific.
fn write_baseline_script(workspace: &Path, name: &str, marker: &str) {
    let body = format!(
        "#!/usr/bin/env bash\n\
         set -euo pipefail\n\n\
         # OMAKURE_SCHEMA_START\n\
         # {{ \"Name\": \"{stem}\", \"Description\": \"baseline member\", \"Fields\": [] }}\n\
         # OMAKURE_SCHEMA_END\n\n\
         printf '{marker}\\n'\n",
        stem = name.trim_end_matches(".sh"),
    );
    let path = workspace.join(name);
    std::fs::write(&path, body).expect("write baseline script");
    support::set_executable(&path);
}

/// Bind the direct listener, point at the peer, and declare the baseline policy.
///
/// Both sides get `direct_bind` and a `static_peers` entry: which of the two
/// actually dials is decided by node id ordering, not by configuration, so a
/// test that configured only one side would pass or hang depending on the keys
/// it happened to generate.
fn configure(
    workspace: &Path,
    direct_port: u16,
    peer: Option<(&str, u16)>,
    allow_push: bool,
    publisher: Option<(&str, &str)>,
) {
    let path = workspace.join("node.toml");
    let config = std::fs::read_to_string(&path).expect("read node config");
    let publishers = match publisher {
        Some((key_id, public_key)) => {
            format!("[{{ key_id = \"{key_id}\", public_key = \"{public_key}\", revoked = false }}]")
        }
        None => "[]".to_string(),
    };
    // `allow_baseline_push = true` with `enrollment = "disabled"` is refused by
    // the pre-existing rule that remote capabilities require enrollment to be
    // enabled. The node reports that only by failing to start.
    let config = config
        .replace("enrollment = \"disabled\"", "enrollment = \"manual\"")
        .replace(
            "static_peers = []",
            &format!(
                "direct_bind = \"127.0.0.1:{direct_port}\"\nstatic_peers = [{}]",
                match peer {
                    Some((node_id, port)) => format!("\"{node_id}@127.0.0.1:{port}\""),
                    None => String::new(),
                }
            ),
        )
        .replace(
            "allow_baseline_push = false",
            &format!("allow_baseline_push = {allow_push}"),
        )
        .replace(
            "baseline_publishers = []",
            &format!("baseline_publishers = {publishers}"),
        )
        .replace("id = \"\"", &format!("id = \"{ORGANIZATION}\""));
    assert!(
        config.contains("enrollment = \"manual\"")
            && config.contains(&format!("direct_bind = \"127.0.0.1:{direct_port}\""))
            && config.contains(&format!("allow_baseline_push = {allow_push}"))
            && config.contains(&format!("id = \"{ORGANIZATION}\"")),
        "the config edit matched nothing -- `node init` changed its output:\n{config}"
    );
    std::fs::write(path, config).expect("write node config");
}

fn wait_for_standing_session(service: &support::HttpServer) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let status = service.get("/v1/node/status");
        if status.status == 200 {
            let transport = status.json()["data"]["transport"].clone();
            let expected = transport["expected_peer_count"].as_u64();
            if expected.is_some_and(|expected| {
                expected > 0
                    && transport["expected_connected_peer_count"].as_u64() == Some(expected)
            }) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// Two real nodes, trusted both ways, with a published baseline the Conductor
/// holds the bodies for.
///
/// Shared so the tests below differ in the one thing each is measuring rather
/// than in the fleet they measure it on.
struct Fleet {
    /// Never serves: a node holding Conductor authority over anyone is refused
    /// a publisher key outright, so the key lives with a third principal.
    _publisher: tempfile::TempDir,
    conductor: tempfile::TempDir,
    performer: tempfile::TempDir,
    performer_id: String,
    baseline_id: String,
    manifest: std::path::PathBuf,
    /// Script names in manifest order.
    scripts: Vec<String>,
}

fn stand_up_fleet() -> Fleet {
    let publisher_dir = tempfile::tempdir().expect("publisher workspace");
    let conductor_dir = tempfile::tempdir().expect("conductor workspace");
    let performer_dir = tempfile::tempdir().expect("performer workspace");
    let publisher = publisher_dir.path();
    let conductor = conductor_dir.path();
    let performer = performer_dir.path();

    init_node(publisher);
    configure(
        publisher,
        support::unique_loopback_port(),
        None,
        false,
        None,
    );
    let key = assert_success_named(
        "baseline create-key",
        &run_node(
            publisher,
            &["baseline".to_string(), "create-key".to_string()],
        ),
    );
    let key_id = key["key_id"].as_str().expect("key id").to_string();
    let public_key = key["public_key"].as_str().expect("public key").to_string();

    write_baseline_script(publisher, "base-a.sh", "base-a v1");
    write_baseline_script(publisher, "base-b.sh", "base-b v1");
    let manifest_path = publisher.join("base-v1.omb");
    let published = assert_success_named(
        "baseline publish",
        &run_node(
            publisher,
            &[
                "baseline".to_string(),
                "publish".to_string(),
                "--script".to_string(),
                "base-a.sh".to_string(),
                "--script".to_string(),
                "base-b.sh".to_string(),
                "--lifetime-seconds".to_string(),
                "3600".to_string(),
                "--out".to_string(),
                manifest_path.display().to_string(),
            ],
        ),
    );
    let baseline_id = published["baseline_id"]
        .as_str()
        .expect("baseline id")
        .to_string();

    let conductor_status = init_node(conductor);
    let performer_status = init_node(performer);
    let conductor_id = conductor_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let performer_id = performer_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let conductor_port = support::unique_loopback_port();
    let performer_port = support::unique_loopback_port();

    configure(
        conductor,
        conductor_port,
        Some((&performer_id, performer_port)),
        false,
        None,
    );
    configure(
        performer,
        performer_port,
        Some((&conductor_id, conductor_port)),
        true,
        Some((key_id.as_str(), public_key.as_str())),
    );

    trust_peer(
        conductor,
        performer,
        &performer_status,
        "performer",
        &["inventory-health", "notifications"],
    );
    trust_peer(
        performer,
        conductor,
        &conductor_status,
        "conductor",
        &["baseline-push", "inventory-health", "notifications"],
    );

    // The Conductor sends the bodies, so it must hold the same bytes the
    // manifest recorded, at the same relative paths.
    write_baseline_script(conductor, "base-a.sh", "base-a v1");
    write_baseline_script(conductor, "base-b.sh", "base-b v1");
    let conductor_manifest = conductor.join("base-v1.omb");
    std::fs::copy(&manifest_path, &conductor_manifest).expect("copy manifest to conductor");

    Fleet {
        _publisher: publisher_dir,
        conductor: conductor_dir,
        performer: performer_dir,
        performer_id,
        baseline_id,
        manifest: conductor_manifest,
        scripts: vec!["base-a.sh".to_string(), "base-b.sh".to_string()],
    }
}

/// The baseline record the Performer wrote, or a failure that says the push
/// never arrived rather than one that says the ack did not.
fn installed_baseline(fleet: &Fleet) -> Value {
    let raw = std::fs::read_to_string(fleet.performer.path().join(".omakure/baseline.json"))
        .expect("the Performer installed no baseline at all");
    serde_json::from_str(&raw).expect("parse installed baseline record")
}

/// A Conductor must be told what became of the baseline it pushed.
///
/// The push itself is only half of the delivery. The other half is the
/// `baseline_ack` coming back over the same standing session, because that ack
/// is the *entire* operator-visible result: `node baseline push` reports
/// `answered` and `accepted` and nothing else. If the ack never lands, a push
/// that installed perfectly reports exactly what a push refused on trust,
/// role, or capability reports -- and the operator's only recourse is to read
/// the receiving node's audit table by hand.
///
/// Asserted on both sides: the Conductor's own reported outcome, and the
/// Performer's installed baseline record.
#[test]
#[ignore = "spawns two real node services; run explicitly"]
fn a_pushed_baseline_is_acknowledged_to_the_conductor() {
    let fleet = stand_up_fleet();
    let performer_service = serve(fleet.performer.path());
    let conductor_service = serve(fleet.conductor.path());
    assert!(
        wait_for_standing_session(&conductor_service),
        "the Conductor never established its standing session with the Performer"
    );

    let pushed = assert_success_named(
        "baseline push",
        &run_node(
            fleet.conductor.path(),
            &[
                "baseline".to_string(),
                "push".to_string(),
                "--peer-node-id".to_string(),
                fleet.performer_id.clone(),
                "--manifest".to_string(),
                fleet.manifest.display().to_string(),
                "--wait-seconds".to_string(),
                "60".to_string(),
            ],
        ),
    );

    // The Performer's own record first: without this, a failing `answered`
    // assertion below could equally mean the push never arrived.
    let installed = installed_baseline(&fleet);
    assert_eq!(
        installed["baseline_id"], fleet.baseline_id,
        "the Performer installed a different baseline than the one pushed"
    );

    assert_eq!(
        pushed["answered"], true,
        "the Performer installed the baseline ({}) but the Conductor was \
         never told: the baseline_ack did not come back over the standing session, so a \
         successful push is indistinguishable from one refused on trust, role, or \
         capability. Reported outcome: {pushed}",
        fleet.baseline_id
    );
    assert_eq!(
        pushed["accepted"], true,
        "the Conductor was answered but told the baseline was refused, while the \
         Performer's own record shows it installed. Reported outcome: {pushed}"
    );
    assert_eq!(pushed["code"], 0, "reported outcome: {pushed}");
    assert_eq!(pushed["baseline_id"], fleet.baseline_id);

    drop(conductor_service);
    drop(performer_service);
}

/// Every `baseline_*` audit row this node wrote, as `(event_type, outcome,
/// error_code)`.
fn baseline_audit_rows(workspace: &Path) -> Vec<(String, String, Option<i64>)> {
    let connection = rusqlite::Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open node registry");
    let mut statement = connection
        .prepare(
            "SELECT event_type, outcome, error_code FROM transport_audit
             WHERE event_type LIKE 'baseline%' ORDER BY id",
        )
        .expect("prepare baseline audit query");
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query baseline audit rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("read baseline audit rows");
    rows
}

/// An ack that misses the caller's budget must be recorded as what it was.
///
/// The budget bounds how long the *caller* waits, and nothing about it says the
/// Performer will stay quiet. On a slow link the ack has arrived a minute or
/// two after the Conductor was already told `answered: false`, with the
/// baseline installed. A session that had forgotten the id hands that ack to
/// the receive half, which judges `baseline_push` messages and can only call a
/// `baseline_ack` malformed -- so the Conductor's own audit table records
/// `baseline_rejected` / 1304 for a baseline the Performer accepted, and the
/// audit trail says the opposite of what happened. That table is the only place
/// an operator can tell "retry it" from "you already have it" apart.
///
/// `wait_seconds = 0` makes the miss deterministic: the budget is spent before
/// the push is even written, so any ack at all is a late one.
#[test]
#[ignore = "spawns two real node services; run explicitly"]
fn a_baseline_ack_that_misses_the_budget_is_recorded_as_what_it_was() {
    let fleet = stand_up_fleet();
    let performer_service = serve(fleet.performer.path());
    let conductor_service = serve(fleet.conductor.path());
    assert!(
        wait_for_standing_session(&conductor_service),
        "the Conductor never established its standing session with the Performer"
    );

    let scripts = fleet
        .scripts
        .iter()
        .map(|name| {
            hex(&std::fs::read(fleet.conductor.path().join(name)).expect("read baseline script"))
        })
        .collect::<Vec<_>>();
    let response = conductor_service.post_json(
        "/v1/node/baselines",
        &serde_json::json!({
            "peer_node_id": fleet.performer_id,
            "manifest": hex(&std::fs::read(&fleet.manifest).expect("read manifest")),
            "scripts": scripts,
            "wait_seconds": 0,
        }),
    );
    assert_eq!(response.status, 200, "push response: {}", response.body);
    let pushed = response.json()["data"].clone();
    assert_eq!(
        pushed["answered"], false,
        "wait_seconds = 0 must spend the budget before the push is written, or this \
         test is not measuring a late ack at all. Reported outcome: {pushed}"
    );

    // The ack cannot be late until it exists. The install is what proves the
    // Performer accepted, which is what the audit row below must agree with.
    let deadline = Instant::now() + Duration::from_secs(30);
    let installed = loop {
        if fleet
            .performer
            .path()
            .join(".omakure/baseline.json")
            .exists()
        {
            break installed_baseline(&fleet);
        }
        assert!(
            Instant::now() < deadline,
            "the Performer never installed the baseline, so no ack was ever sent"
        );
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(installed["baseline_id"], fleet.baseline_id);

    let mut rows = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        rows = baseline_audit_rows(fleet.conductor.path());
        if rows
            .iter()
            .any(|(event, _, _)| event == "baseline_answered_late")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    assert!(
        rows.contains(&(
            "baseline_answered_late".to_string(),
            "accepted".to_string(),
            None
        )),
        "the Performer accepted the baseline and said so, but the Conductor recorded \
         no late answer: an operator reading this table cannot tell an install that \
         landed from one that never did. Rows: {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(event, outcome, _)| event == "baseline_rejected" && outcome == "rejected"),
        "the Conductor audited its own peer's acceptance as a rejection: the late ack \
         fell through to the receive half, which can only read a `baseline_ack` as a \
         malformed `baseline_push`. Rows: {rows:?}"
    );

    drop(conductor_service);
    drop(performer_service);
}
