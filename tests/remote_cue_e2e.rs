//! One authorized Cue, end to end, between two real nodes.
//!
//! Everything below this point has been proven in isolation: the gates, the
//! resolution, the deny-all policy, the at-most-once guard. What none of it
//! proves is that the pieces meet — that a Cue minted by one `omakure` process
//! crosses a real Noise session, is authorized by another process reading its
//! own config and registry, and results in exactly one run of exactly the
//! declared script.
//!
//! Two real `node serve` processes, real transport, no mocks.

mod support;

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const TOKEN: &str = "remote-cue-e2e-token-with-enough-entropy-000001";
/// A Cue that is going to be authorized should be decided in well under this.
const CUE_EFFECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The Signal rides the Performer's standing reporting session, which has its
/// own tick, so this is deliberately looser than the effect timeout.
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(90);

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
    // Carried on the failure message: a test that only reports "a node command
    // failed" costs a bisect every time it goes red.
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
        "remote-cue-e2e".to_string(),
        "--reason".to_string(),
        "remote cue certification".to_string(),
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

/// Bind the direct listener, point at the peer, and declare the remote policy.
///
/// The declaration is the point of the test as much as the transport is: a node
/// that has not written down what it will run must refuse, and one that has
/// must accept exactly that.
fn configure(
    workspace: &Path,
    direct_port: u16,
    peer: Option<(&str, u16)>,
    allow_cues: bool,
    declared: &[&str],
) {
    let path = workspace.join("node.toml");
    let config = std::fs::read_to_string(&path).expect("read node config");
    let declared_list = declared
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    // Each key is rewritten in place. `node init` already emits every one of
    // them, so inserting rather than replacing would produce a duplicate key
    // and an unparseable config -- which the node reports only by refusing to
    // start, several seconds later and nowhere near the cause.
    // `allow_remote_cues = true` with `enrollment = "disabled"` is refused by a
    // pre-existing rule: remote capabilities require enrollment enabled. The
    // node reports it only by failing to start, so it is set here deliberately
    // rather than discovered again.
    let config = config
        .replace("enrollment = \"disabled\"", "enrollment = \"manual\"")
        .replace(
            "static_peers = []",
            // Static peers point one way only. A Cue is a one-shot dial with
            // an explicit endpoint, and if the *Conductor* also held a standing
            // session to the Performer it would already occupy the single
            // connection that peer is allowed, so the dispatch would be refused
            // for a reason that has nothing to do with Cues. The Performer's
            // standing session is the one that matters: it is how the outcome
            // Signal gets home.
            &format!(
                "direct_bind = \"127.0.0.1:{direct_port}\"\nstatic_peers = [{}]",
                match peer {
                    Some((node_id, port)) => format!("\"{node_id}@127.0.0.1:{port}\""),
                    None => String::new(),
                }
            ),
        )
        .replace(
            "allow_remote_cues = false",
            &format!("allow_remote_cues = {allow_cues}"),
        )
        .replace(
            "remote_cue_scripts = []",
            &format!("remote_cue_scripts = [{declared_list}]"),
        );
    assert!(
        config.contains("enrollment = \"manual\"")
            && config.contains(&format!("direct_bind = \"127.0.0.1:{direct_port}\""))
            && config.contains(&format!("allow_remote_cues = {allow_cues}"))
            && config.contains(&format!("remote_cue_scripts = [{declared_list}]")),
        "the config edit matched nothing -- `node init` changed its output:\n{config}"
    );
    std::fs::write(path, config).expect("write node config");
}

/// The script whose execution is the observable effect.
///
/// It appends to a file rather than printing, so "did it run, and how many
/// times" is answered by counting lines on disk instead of by trusting a row
/// state to describe reality.
fn write_effect_script(workspace: &Path, marker: &Path) {
    let script = workspace.join("deploy.sh");
    std::fs::write(
        &script,
        format!(
            "#!/usr/bin/env bash\n\
             # OMAKURE_SCHEMA_START\n\
             # {{\"Name\":\"deploy\",\"Description\":\"e2e effect\",\"Fields\":[]}}\n\
             # OMAKURE_SCHEMA_END\n\
             echo ran >> {}\n",
            marker.display()
        ),
    )
    .expect("write the effect script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
}

fn effect_count(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .map(|body| body.lines().filter(|line| !line.is_empty()).count())
        .unwrap_or(0)
}

fn wait_for_effect(marker: &Path, expected: usize) -> bool {
    let deadline = Instant::now() + CUE_EFFECT_TIMEOUT;
    while Instant::now() < deadline {
        if effect_count(marker) >= expected {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// The whole feature, on two real nodes.
///
/// A Conductor dispatches one Cue over a real Noise session. The Performer
/// authorizes it against its own config and registry, runs the declared script
/// once, and the effect appears on disk.
#[test]
#[ignore = "spawns two real node services; run explicitly"]
fn an_authorized_cue_runs_the_declared_script_exactly_once() {
    let conductor_dir = tempfile::tempdir().expect("conductor workspace");
    let performer_dir = tempfile::tempdir().expect("performer workspace");
    let conductor = conductor_dir.path();
    let performer = performer_dir.path();

    let conductor_port = support::unique_loopback_port();
    let performer_port = support::unique_loopback_port();

    let conductor_status = init_node(conductor);
    let performer_status = init_node(performer);
    let performer_id = performer_status["identity"]["node_id"].as_str().unwrap();

    let marker = performer.join("effects.log");
    write_effect_script(performer, &marker);

    // The Performer trusts the Conductor with exactly the two capabilities the
    // gates require, and declares exactly one script as remotely runnable.
    trust_peer(
        performer,
        conductor,
        &conductor_status,
        "conductor",
        &["inventory-health", "notifications", "remote-run"],
    );
    trust_peer(
        conductor,
        performer,
        &performer_status,
        "performer",
        &["inventory-health", "notifications", "remote-run"],
    );
    // No static peer on either side, and that is a limitation rather than a
    // preference: a Performer that already holds a session with this Conductor
    // refuses the one-shot dial with `1010`, because the responder must not
    // accept from a peer it owns the dial to. See the note above
    // `wait_for_signal`.
    configure(performer, performer_port, None, true, &["deploy.sh"]);
    configure(conductor, conductor_port, None, false, &[]);

    let _performer_service = serve(performer);
    let _conductor_service = serve(conductor);

    assert_eq!(effect_count(&marker), 0, "nothing has run yet");

    let dispatched = assert_success_named(
        "cue",
        &run_node(
            conductor,
            &[
                "cue".to_string(),
                "--endpoint".to_string(),
                format!("127.0.0.1:{performer_port}"),
                "--peer-node-id".to_string(),
                performer_id.to_string(),
                "--script".to_string(),
                "deploy.sh".to_string(),
                "--reason".to_string(),
                "end to end certification".to_string(),
            ],
        ),
    );
    assert!(
        dispatched["cue_id"]
            .as_str()
            .is_some_and(|id| id.len() == 32),
        "the dispatcher must return the minted cue id: {dispatched}"
    );
    assert_eq!(
        (
            dispatched["answered"].as_bool(),
            dispatched["accepted"].as_bool(),
            dispatched["code"].as_u64()
        ),
        (Some(true), Some(true), Some(0)),
        "the Performer must answer, and answer accepted: {dispatched}"
    );

    assert!(
        wait_for_effect(&marker, 1),
        "the declared script never ran on the Performer"
    );

    // Give any duplicate a chance to appear before asserting there is none.
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        effect_count(&marker),
        1,
        "an authorized Cue must run the script exactly once"
    );

    // The correlation half: the Conductor derives the run id it will see from
    // the cue id it minted, with no message carrying a correlation field. The
    // *derivation* is asserted here; that the Signal arrives is not yet, and
    // the reason is a real limitation rather than an oversight -- see below.
    let expected = dispatched["expected_run_id"]
        .as_str()
        .expect("the dispatcher must return the run id it will look for")
        .to_string();
    assert_eq!(expected.len(), 32, "an opaque run id is 16 bytes of hex");
    assert_eq!(
        expected,
        derived_run_id(dispatched["cue_id"].as_str().unwrap()),
        "both sides must derive the same run id from the same cue id"
    );
}

/// The Conductor-side derivation, computed independently of the dispatcher.
///
/// Recomputed here rather than read back, so the assertion compares two
/// derivations instead of comparing a value to itself.
fn derived_run_id(cue_id: &str) -> String {
    omakure::health_plane::report::opaque_run_id(&omakure::remote_cue::derive_run_id(cue_id))
}

/// Poll the Conductor's own Signal feed for a `run-completed` carrying this id.
///
/// Read through the shipped CLI rather than the database, so what is asserted
/// is what an operator would actually see.
///
/// **Not yet called, and the reason is load-bearing.** Delivering the outcome
/// needs a standing session between the two nodes, and a Performer holding one
/// with this Conductor refuses the one-shot Cue dial with `1010`: `register`
/// rejects a Responder connection from a peer it owns the dial to, and rejects
/// any second connection to a peer it already has. So the configuration that
/// delivers the Signal is exactly the configuration in which a Cue cannot be
/// sent. `omakure node direct-probe` shares the collision by construction --
/// both enter through the same probe ritual. Raised for an owner decision.
#[allow(dead_code)]
fn wait_for_signal(conductor: &Path, expected_run_id: &str) -> bool {
    let deadline = Instant::now() + SIGNAL_TIMEOUT;
    while Instant::now() < deadline {
        let feed = run_node(conductor, &["signals".to_string()]);
        if feed.status.success() {
            let envelope = support::json_envelope(&feed.stdout);
            if let Some(signals) = envelope["data"]["signals"].as_array() {
                if signals.iter().any(|entry| {
                    entry["kind"] == "run-completed" && entry["run"]["run_id"] == expected_run_id
                }) {
                    return true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// The refusal half, on the same real topology.
///
/// Everything is trusted and capable; only the declaration is missing. This is
/// the property the owner asked for — what may run is written down — and it is
/// worth proving on real nodes rather than only in a unit test, because it is
/// the difference between a config field and an enforced one.
#[test]
#[ignore = "spawns two real node services; run explicitly"]
fn an_undeclared_script_is_refused_by_a_fully_trusted_conductor() {
    let conductor_dir = tempfile::tempdir().expect("conductor workspace");
    let performer_dir = tempfile::tempdir().expect("performer workspace");
    let conductor = conductor_dir.path();
    let performer = performer_dir.path();

    let conductor_port = support::unique_loopback_port();
    let performer_port = support::unique_loopback_port();

    let conductor_status = init_node(conductor);
    let performer_status = init_node(performer);
    let performer_id = performer_status["identity"]["node_id"].as_str().unwrap();

    let marker = performer.join("effects.log");
    write_effect_script(performer, &marker);

    trust_peer(
        performer,
        conductor,
        &conductor_status,
        "conductor",
        &["inventory-health", "notifications", "remote-run"],
    );
    trust_peer(
        conductor,
        performer,
        &performer_status,
        "performer",
        &["inventory-health", "notifications", "remote-run"],
    );
    // Cues enabled, full trust, full capabilities — and nothing declared.
    configure(performer, performer_port, None, true, &[]);
    configure(conductor, conductor_port, None, false, &[]);

    let _performer_service = serve(performer);
    let _conductor_service = serve(conductor);

    assert_success_named(
        "cue",
        &run_node(
            conductor,
            &[
                "cue".to_string(),
                "--endpoint".to_string(),
                format!("127.0.0.1:{performer_port}"),
                "--peer-node-id".to_string(),
                performer_id.to_string(),
                "--script".to_string(),
                "deploy.sh".to_string(),
                "--reason".to_string(),
                "should be refused".to_string(),
            ],
        ),
    );

    // The dispatch itself succeeds — it is one-shot and does not wait for a
    // verdict. What must not happen is the script running.
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(
        effect_count(&marker),
        0,
        "an undeclared script must not run, however trusted the sender is"
    );
}
