use crate::baseline::{BaselinePublisherKey, SignedBaselineManifest};
use crate::baseline_push::{
    BaselineCode, BaselineOutcome, BaselinePolicy, BaselinePush, BaselineSession,
};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerRole};
use crate::util::hex;
use crate::workspace::Workspace;
use std::path::Path;

const ISSUED_AT: u64 = 1_800_000_000;
const NOW: u64 = 1_800_000_060;
const LIFETIME: u64 = 3_600;

/// One node's private state: identity, registry, workspace.
struct Performer {
    _dir: tempfile::TempDir,
    context: NodeContext,
    workspace: Workspace,
}

impl Performer {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = crate::test_support::configured_node_context(dir.path());
        let workspace = Workspace::new(dir.path().join("scripts"));
        workspace.ensure_layout().expect("workspace layout");
        Self {
            _dir: dir,
            context,
            workspace,
        }
    }

    fn identity(&self) -> NodeIdentity {
        NodeIdentity::load_or_initialize(&self.context).expect("identity")
    }

    fn registry(&self, identity: &NodeIdentity) -> NodeRegistry {
        NodeRegistry::open_for_initialization(&self.context, identity.public_status())
            .expect("registry")
    }

    fn installed(&self, relative: &str) -> Option<Vec<u8>> {
        std::fs::read(self.workspace.scripts_root().join(relative)).ok()
    }
}

/// A publisher the receiver may or may not name.
fn publisher(scalar: u8) -> (k256::schnorr::SigningKey, BaselinePublisherKey) {
    use sha2::Digest;
    let signing_key = k256::schnorr::SigningKey::from_slice(&[scalar; 32]).expect("scalar");
    let mut public_key = [0u8; 32];
    public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
    let mut key_id = [0u8; 16];
    key_id.copy_from_slice(&sha2::Sha256::digest(public_key)[..16]);
    (
        signing_key,
        BaselinePublisherKey {
            key_id,
            public_key,
            revoked: false,
        },
    )
}

fn fleet_scripts() -> Vec<(String, Vec<u8>)> {
    vec![
        (
            "ops/deploy.sh".to_string(),
            b"#!/bin/sh\necho fleet-deploy\n".to_vec(),
        ),
        (
            "audit.sh".to_string(),
            b"#!/bin/sh\necho fleet-audit\n".to_vec(),
        ),
    ]
}

fn sign(scalar: u8, bodies: &[(String, Vec<u8>)]) -> SignedBaselineManifest {
    let (signing_key, key) = publisher(scalar);
    SignedBaselineManifest::sign_with_material(
        signing_key.to_bytes().as_ref(),
        key.key_id,
        "acme".to_string(),
        bodies,
        ISSUED_AT,
        ISSUED_AT + LIFETIME,
    )
    .expect("sign the baseline")
}

/// Build the wire payload the way the sender does: bodies in manifest order.
fn wire(manifest: &SignedBaselineManifest, bodies: &[(String, Vec<u8>)]) -> serde_json::Value {
    let ordered: Vec<Vec<u8>> = manifest
        .entries
        .iter()
        .map(|entry| {
            bodies
                .iter()
                .find(|(path, _)| path == &entry.path)
                .map(|(_, body)| body.clone())
                .unwrap_or_default()
        })
        .collect();
    BaselinePush::encode(&manifest.encode(), &ordered)
}

fn policy(enabled: bool, publishers: Vec<BaselinePublisherKey>) -> BaselinePolicy {
    BaselinePolicy {
        enabled,
        publishers,
        organization: "acme".to_string(),
    }
}

/// Record the sender as an active Conductor holding `baseline-push`.
fn trust_conductor(registry: &NodeRegistry, node_id: &str, public_key: &str, capability: &str) {
    registry
        .import_manual_peer_with_transport(
            crate::node_registry::PeerRegistration {
                node_id: node_id.to_string(),
                public_key: public_key.to_string(),
                role: PeerRole::Conductor,
                capabilities: vec![capability.to_string()],
                source: crate::node_registry::PeerSource::Manual,
                actor: "test".to_string(),
                reason: "baseline delivery test".to_string(),
            },
            None,
        )
        .expect("record the conductor");
}

/// Drive one push through a real session and report what the workspace holds.
fn deliver(
    performer: &Performer,
    conductor: &Performer,
    policy: BaselinePolicy,
    payload: serde_json::Value,
) -> BaselineOutcome {
    let performer_identity = performer.identity();
    let registry = performer.registry(&performer_identity);
    let conductor_identity = conductor.identity();
    let conductor_status = conductor_identity.public_status();

    trust_conductor(
        &registry,
        &conductor_status.node_id,
        &conductor_status.public_key_hex,
        crate::baseline_push::CAPABILITY_BASELINE_PUSH,
    );
    deliver_with_registry(
        performer,
        &registry,
        &performer_identity,
        &conductor_identity,
        policy,
        payload,
    )
}

fn deliver_with_registry(
    performer: &Performer,
    registry: &NodeRegistry,
    performer_identity: &NodeIdentity,
    conductor_identity: &NodeIdentity,
    policy: BaselinePolicy,
    payload: serde_json::Value,
) -> BaselineOutcome {
    let session_id = [42u8; 32];
    let mut nonce = [0u8; 16];
    nonce[0] = 7;
    let conductor_status = conductor_identity.public_status();
    let conductor_key: [u8; 32] = hex::decode_array(&conductor_status.public_key_hex).expect("hex");

    let envelope = crate::direct_transport::sign_baseline_envelope(
        conductor_identity,
        crate::baseline_push::KIND_PUSH,
        &session_id,
        nonce,
        payload,
        NOW,
    )
    .expect("sign the push");

    let mut session = BaselineSession::new(
        registry,
        performer_identity,
        &conductor_status.node_id,
        conductor_key,
        session_id,
        policy,
        Some(Workspace::new(performer.workspace.root().to_path_buf())),
    );
    session.handle_envelope(&envelope.encoded(), NOW)
}

fn accepted(outcome: &BaselineOutcome) -> bool {
    matches!(
        outcome,
        BaselineOutcome::Decided(crate::baseline_push::BaselineDecision::Accepted { .. })
    )
}

fn refused_with(outcome: &BaselineOutcome) -> Option<BaselineCode> {
    match outcome {
        BaselineOutcome::Decided(crate::baseline_push::BaselineDecision::Rejected(code)) => {
            Some(*code)
        }
        _ => None,
    }
}

/// Acceptance: a Performer with the gate on, naming this publisher, installs
/// the set, and the scripts are runnable afterwards.
#[test]
fn a_named_publishers_baseline_installs_and_the_scripts_run() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);

    let outcome = deliver(
        &performer,
        &conductor,
        policy(true, vec![publisher(3).1]),
        wire(&manifest, &bodies),
    );

    assert!(accepted(&outcome), "got {outcome:?}");
    for (path, body) in &bodies {
        assert_eq!(
            performer.installed(path).as_deref(),
            Some(body.as_slice()),
            "{path} must be on disk with the published bytes"
        );
    }

    // "Runnable" is asserted by running one, not by reading a mode bit: a file
    // whose permissions look right and whose interpreter cannot start it is
    // still not a script anybody can use.
    let installed = performer.workspace.scripts_root().join("audit.sh");
    let output = std::process::Command::new("/bin/sh")
        .arg(&installed)
        .output()
        .expect("run the installed script");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "fleet-audit"
    );
}

/// A push repeated on one session must be answered, not met with silence.
///
/// `Duplicate` is reportable and `Repeat` is documented as "answered from
/// the first decision", but the repeat was audited and then dropped without
/// queueing an ack. The ack is the only thing a Conductor ever sees, so the
/// push sat until `--wait-seconds` ran out and then reported
/// `answered: false` -- byte for byte what a push refused on trust, role,
/// or capability reports. On two real VMs a duplicate push burned the full
/// 60-second budget and was indistinguishable from a silently refused one;
/// telling them apart meant reading the Performer's audit table by hand.
#[test]
fn a_repeated_push_is_answered_rather_than_silently_dropped() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);
    let payload = wire(&manifest, &bodies);

    let performer_identity = performer.identity();
    let registry = performer.registry(&performer_identity);
    let conductor_identity = conductor.identity();
    let conductor_status = conductor_identity.public_status();
    trust_conductor(
        &registry,
        &conductor_status.node_id,
        &conductor_status.public_key_hex,
        crate::baseline_push::CAPABILITY_BASELINE_PUSH,
    );

    let session_id = [42u8; 32];
    let mut nonce = [0u8; 16];
    nonce[0] = 7;
    let conductor_key: [u8; 32] = hex::decode_array(&conductor_status.public_key_hex).expect("hex");
    let envelope = crate::direct_transport::sign_baseline_envelope(
        &conductor_identity,
        crate::baseline_push::KIND_PUSH,
        &session_id,
        nonce,
        payload,
        NOW,
    )
    .expect("sign the push");

    // One session, deliberately: the duplicate guard is per-session, so a
    // second session would install again rather than exercise the repeat.
    let mut session = BaselineSession::new(
        &registry,
        &performer_identity,
        &conductor_status.node_id,
        conductor_key,
        session_id,
        policy(true, vec![publisher(3).1]),
        Some(Workspace::new(performer.workspace.root().to_path_buf())),
    );

    let first = session.handle_envelope(&envelope.encoded(), NOW);
    assert!(accepted(&first), "the first push must install: {first:?}");
    assert!(
        session.take_reply().is_some(),
        "the first push must be acked"
    );

    let second = session.handle_envelope(&envelope.encoded(), NOW);
    assert_eq!(second, BaselineOutcome::Repeat, "got {second:?}");
    let reply = session.take_reply().expect(
        "a repeated push was audited but never answered: the Conductor is left waiting \
             out its whole budget and cannot tell a duplicate from a silent refusal",
    );
    let view = crate::direct_transport::envelope_view(&reply).expect("decode the ack");
    assert_eq!(view.payload["accepted"], false, "ack: {}", view.payload);
    assert_eq!(
        view.payload["error"]["code"],
        u64::from(BaselineCode::Duplicate.code()),
        "the repeat must name itself as {} rather than any other refusal: {}",
        BaselineCode::Duplicate.name(),
        view.payload
    );
}

/// Acceptance: an unnamed publisher changes nothing, and the workspace is what
/// proves it.
#[test]
fn an_unnamed_publisher_changes_nothing_on_disk() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(9, &bodies);

    // Seeded so "nothing changed" is a statement about content, not about
    // absence: a workspace that was empty before and after would pass even if
    // the install had silently been skipped for an unrelated reason.
    std::fs::create_dir_all(performer.workspace.scripts_root().join("ops")).expect("mkdir");
    std::fs::write(
        performer.workspace.scripts_root().join("ops/deploy.sh"),
        b"#!/bin/sh\necho the-operators-own\n",
    )
    .expect("seed");

    let outcome = deliver(
        &performer,
        &conductor,
        policy(true, vec![publisher(3).1]),
        wire(&manifest, &bodies),
    );

    assert_eq!(refused_with(&outcome), Some(BaselineCode::PublisherUnknown));
    assert_eq!(
        performer.installed("ops/deploy.sh").as_deref(),
        Some(b"#!/bin/sh\necho the-operators-own\n".as_slice()),
        "the operator's own script must be untouched"
    );
    assert!(performer.installed("audit.sh").is_none());
}

/// Acceptance: a revoked publisher changes nothing either. Revocation that
/// only affected the reply would be advisory.
#[test]
fn a_revoked_publisher_changes_nothing_on_disk() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);
    let revoked = BaselinePublisherKey {
        revoked: true,
        ..publisher(3).1
    };

    let outcome = deliver(
        &performer,
        &conductor,
        policy(true, vec![revoked]),
        wire(&manifest, &bodies),
    );

    assert_eq!(refused_with(&outcome), Some(BaselineCode::PublisherRevoked));
    assert!(performer.installed("ops/deploy.sh").is_none());
    assert!(performer.installed("audit.sh").is_none());
}

/// Acceptance: a manifest whose recorded hash does not match its script is
/// refused as a whole — including the scripts that *did* match.
#[test]
fn one_mismatched_script_installs_none_of_the_set() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);

    let mut tampered = bodies.clone();
    tampered
        .iter_mut()
        .find(|(path, _)| path == "ops/deploy.sh")
        .expect("entry")
        .1
        .extend_from_slice(b"echo also-this\n");

    let outcome = deliver(
        &performer,
        &conductor,
        policy(true, vec![publisher(3).1]),
        wire(&manifest, &tampered),
    );

    assert_eq!(refused_with(&outcome), Some(BaselineCode::ContentMismatch));
    assert!(
        performer.installed("audit.sh").is_none(),
        "the script that matched its hash must not survive the set being void"
    );
    assert!(performer.installed("ops/deploy.sh").is_none());
}

/// Acceptance: with the gate off — the shipped default — nothing installs and
/// the node keeps serving.
///
/// The second half is the one `docs/fleet-operations.md` froze for enrollment: refusing
/// a baseline must never mean refusing to serve. It is checked by delivering a
/// second baseline on the same session after the refusal and watching it be
/// decided normally, which a node that had torn the session down could not do.
#[test]
fn with_the_gate_off_nothing_installs_and_the_node_keeps_serving() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);

    let performer_identity = performer.identity();
    let registry = performer.registry(&performer_identity);
    let conductor_identity = conductor.identity();
    let conductor_status = conductor_identity.public_status();
    trust_conductor(
        &registry,
        &conductor_status.node_id,
        &conductor_status.public_key_hex,
        crate::baseline_push::CAPABILITY_BASELINE_PUSH,
    );

    let refused = deliver_with_registry(
        &performer,
        &registry,
        &performer_identity,
        &conductor_identity,
        policy(false, vec![publisher(3).1]),
        wire(&manifest, &bodies),
    );
    assert_eq!(refused_with(&refused), Some(BaselineCode::Disabled));
    assert!(performer.installed("ops/deploy.sh").is_none());
    assert!(performer.installed("audit.sh").is_none());

    // The node is still answering: the same session decides a second baseline
    // rather than having stopped serving over the first refusal.
    let still_serving = deliver_with_registry(
        &performer,
        &registry,
        &performer_identity,
        &conductor_identity,
        policy(true, vec![publisher(3).1]),
        wire(&manifest, &bodies),
    );
    assert!(
        accepted(&still_serving),
        "refusing a baseline must not have stopped this node serving; got {still_serving:?}"
    );
}

/// A peer this node trusts for runs is not thereby trusted to supply them.
#[test]
fn a_conductor_without_the_capability_installs_nothing() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);

    let performer_identity = performer.identity();
    let registry = performer.registry(&performer_identity);
    let conductor_identity = conductor.identity();
    let conductor_status = conductor_identity.public_status();
    trust_conductor(
        &registry,
        &conductor_status.node_id,
        &conductor_status.public_key_hex,
        crate::remote_cue::CAPABILITY_REMOTE_RUN,
    );

    let outcome = deliver_with_registry(
        &performer,
        &registry,
        &performer_identity,
        &conductor_identity,
        policy(true, vec![publisher(3).1]),
        wire(&manifest, &bodies),
    );

    assert_eq!(
        refused_with(&outcome),
        Some(BaselineCode::MissingBaselinePush),
        "ordering a run and supplying what runs are different powers"
    );
    assert!(performer.installed("ops/deploy.sh").is_none());
}

/// The sender's standing is decided before the manifest is looked at.
///
/// Ordering is the property, and it is not observable from whether the
/// install happened -- the standing is re-read before the write too, so
/// either check alone would keep the workspace clean. What only the *first*
/// check produces is silence: a peer this node does not trust must be told
/// nothing about the artefact, not even that its publisher is unknown here.
/// The refusal code is the evidence of which check ran.
#[test]
fn an_untrusted_sender_never_learns_anything_about_the_artefact() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    // Signed by a publisher this node has never named, so the *content*
    // gates have something to say. If they ran, they would say it.
    let manifest = sign(9, &bodies);

    let performer_identity = performer.identity();
    let registry = performer.registry(&performer_identity);
    let conductor_identity = conductor.identity();

    let unknown_peer = deliver_with_registry(
        &performer,
        &registry,
        &performer_identity,
        &conductor_identity,
        policy(true, vec![publisher(3).1]),
        wire(&manifest, &bodies),
    );
    assert_eq!(
        refused_with(&unknown_peer),
        Some(BaselineCode::NotActiveConductor),
        "a peer this node does not know must not receive a verdict on the \
             publisher; that answer belongs to a later gate"
    );
    assert!(
        !BaselineCode::NotActiveConductor.is_reportable(),
        "and the refusal it does get is one it is never told"
    );

    let conductor_status = conductor_identity.public_status();
    trust_conductor(
        &registry,
        &conductor_status.node_id,
        &conductor_status.public_key_hex,
        crate::baseline_push::CAPABILITY_BASELINE_PUSH,
    );
    let trusted_peer = deliver_with_registry(
        &performer,
        &registry,
        &performer_identity,
        &conductor_identity,
        policy(true, vec![publisher(3).1]),
        wire(&manifest, &bodies),
    );
    assert_eq!(
        refused_with(&trusted_peer),
        Some(BaselineCode::PublisherUnknown),
        "the control: once the sender passes, the content gates do answer"
    );
}

/// Trust withdrawn during verification stops the write.
///
/// The window is opened deliberately: the baseline is verified while the
/// peer is trusted, the peer is then revoked, and only then is the install
/// step reached. In production that gap is the time a signature check and
/// N content hashes take, which is real but not something a test can pause
/// inside. Driving the step directly is what makes the check demonstrable
/// instead of merely present.
#[test]
fn a_peer_revoked_during_verification_does_not_get_its_code_installed() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);

    let performer_identity = performer.identity();
    let registry = performer.registry(&performer_identity);
    let conductor_identity = conductor.identity();
    let conductor_status = conductor_identity.public_status();
    trust_conductor(
        &registry,
        &conductor_status.node_id,
        &conductor_status.public_key_hex,
        crate::baseline_push::CAPABILITY_BASELINE_PUSH,
    );

    let policy = policy(true, vec![publisher(3).1]);
    let push = BaselinePush::parse(&wire(&manifest, &bodies)).expect("parse");
    let verified =
        crate::baseline_push::verify_push(&push, &policy, NOW).expect("verified while trusted");
    let baseline_id = verified.baseline_id().expect("id");

    let mut session = BaselineSession::new(
        &registry,
        &performer_identity,
        &conductor_status.node_id,
        [0u8; 32],
        [42u8; 32],
        policy,
        Some(Workspace::new(performer.workspace.root().to_path_buf())),
    );

    // The control: still trusted, so this step does install. Without it a
    // refusal below would prove only that the step never writes anything.
    assert!(accepted(&session.install_when_still_trusted(
        &verified,
        baseline_id,
        NOW
    )));
    std::fs::remove_file(performer.workspace.scripts_root().join("audit.sh"))
        .expect("clear the control");

    registry
        .revoke_peer(
            &conductor_status.node_id,
            "test",
            "revoked mid-verification",
        )
        .expect("revoke");

    let outcome = session.install_when_still_trusted(&verified, baseline_id, NOW);

    assert_eq!(
        refused_with(&outcome),
        Some(BaselineCode::NotActiveConductor),
        "revocation must stop work already in flight, or it is advisory"
    );
    assert!(
        performer.installed("audit.sh").is_none(),
        "the revoked peer's code must not have reached the disk"
    );
}

/// The wire carries no paths, so a sender cannot redirect one script.
///
/// There is no field in the payload in which to say where a script goes; the
/// manifest is the only source. This checks the consequence rather than the
/// absence: an extra body is refused because the manifest names two entries,
/// and nothing about the wire can change what those two entries are.
#[test]
fn the_wire_cannot_name_a_destination_the_publisher_did_not_sign() {
    let performer = Performer::new();
    let conductor = Performer::new();
    let bodies = fleet_scripts();
    let manifest = sign(3, &bodies);

    let mut payload = wire(&manifest, &bodies);
    payload["scripts"]
        .as_array_mut()
        .expect("scripts array")
        .push(serde_json::json!("23"));

    let outcome = deliver(
        &performer,
        &conductor,
        policy(true, vec![publisher(3).1]),
        payload,
    );

    assert_eq!(refused_with(&outcome), Some(BaselineCode::ContentMismatch));
    assert!(performer.installed("ops/deploy.sh").is_none());
}

/// The publish CLI refuses a baseline it knows can never be delivered.
#[test]
fn publishing_more_than_one_push_can_carry_is_refused_at_signing_time() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = crate::test_support::workspace_in(&dir);
    std::fs::write(
        workspace.scripts_root().join("huge.sh"),
        vec![b'x'; crate::baseline_push::MAX_PUSH_SCRIPT_BYTES + 1],
    )
    .expect("write");

    let key_dir = tempfile::tempdir().expect("tempdir");
    let context = crate::test_support::configured_node_context(key_dir.path());
    let identity = NodeIdentity::load_or_initialize(&context).expect("identity");
    let registry = NodeRegistry::open_for_initialization(&context, identity.public_status())
        .expect("registry");
    let publisher =
        crate::baseline_publisher::BaselinePublisher::create(&context, &registry).expect("key");

    let error = crate::operations::baseline::publish_baseline(
        &workspace,
        &publisher,
        "acme",
        &["huge.sh".to_string()],
        ISSUED_AT,
        LIFETIME,
        Path::new(&dir.path().join("manifest.ombm")),
    )
    .expect_err("a baseline that cannot be delivered must not be signed");

    assert!(
        error.to_string().contains("cannot be delivered"),
        "got {error:?}"
    );
    assert!(
        !dir.path().join("manifest.ombm").exists(),
        "a refused publish must not leave a manifest behind"
    );
}
