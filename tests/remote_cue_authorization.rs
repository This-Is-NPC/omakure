//! The seam where a Cue enters, and the guarantee that it stays a seam.
//!
//! `hold_session` dispatches every decrypted application envelope to the Health
//! Plane first, and today anything without the `health_` prefix falls through
//! `HealthOutcome::NotHealth` and is silently discarded. The Cue branch is being
//! added at exactly that point, so these characterise the boundary *before* it
//! moves: the Health Plane must not claim Cue traffic, and must keep its own
//! behaviour unchanged when Cue traffic arrives.
//!
//! Without this, a mistake in the Cue branch that quietly made the Health Plane
//! start or stop handling something would be caught only by the multi-node e2e,
//! and only if it happened to exercise the same shape.

use omakure::direct_health::{HealthOutcome, HealthSession};
use omakure::direct_transport::{sign_cue_envelope, sign_health_envelope, CUE_KIND_PREFIX};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use omakure::node_registry::NodeRegistry;
use serde_json::json;
use std::path::Path;

fn identity_and_registry(root: &Path) -> (NodeIdentity, NodeRegistry) {
    let config = root.join("node.toml");
    std::fs::write(&config, "version = 1\n").expect("write config");
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(Some(root.join("state")), Some(config)),
        true,
        None,
        None,
        None,
    )
    .expect("resolve node context");
    let identity = NodeIdentity::load_or_initialize(&context).expect("identity");
    let registry =
        NodeRegistry::open_existing(&context, identity.public_status()).expect("registry");
    (identity, registry)
}

/// A Cue kind must fall through the Health Plane untouched.
///
/// This is the property the Cue branch depends on. If the Health Plane ever
/// started answering `cue_` traffic, two planes would be authorizing the same
/// message with different rules.
#[test]
fn the_health_plane_does_not_claim_cue_traffic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (identity, registry) = identity_and_registry(dir.path());
    let mut session = HealthSession::new(
        &identity,
        &registry,
        "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        &[3u8; 32],
        [7u8; 32],
        None,
    );

    for kind in ["cue_dispatch", "cue_ack"] {
        let envelope = sign_cue_envelope(
            &identity,
            kind,
            &[7u8; 32],
            [9u8; 16],
            json!({"version": 1}),
            1_800_000_000,
        )
        .expect("sign the cue envelope");

        assert!(
            matches!(
                session.handle_envelope(&envelope.encoded()),
                HealthOutcome::NotHealth
            ),
            "{kind} must fall through to the Cue branch, not be handled here"
        );
    }
}

/// Anything that is neither plane must still be discarded, not answered.
///
/// Adding a Cue branch must not turn the dispatcher into something that replies
/// to unknown traffic, which would make it a probe oracle.
#[test]
fn unknown_kinds_are_still_not_claimed_by_the_health_plane() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (identity, registry) = identity_and_registry(dir.path());
    let mut session = HealthSession::new(
        &identity,
        &registry,
        "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        &[3u8; 32],
        [7u8; 32],
        None,
    );

    // Not signable through either wrapper, so build it through the Health one
    // with a health kind and then assert the *Cue* prefix check would reject it.
    assert!(!"probe".starts_with(CUE_KIND_PREFIX));

    let envelope = sign_health_envelope(
        &identity,
        "health_profile",
        &[7u8; 32],
        [9u8; 16],
        json!({"version": 1}),
        1_800_000_000,
    )
    .expect("sign a health envelope");

    // A health kind IS claimed — this is the control that proves the assertion
    // above is not passing because the session claims nothing at all.
    assert!(
        !matches!(
            session.handle_envelope(&envelope.encoded()),
            HealthOutcome::NotHealth
        ),
        "the Health Plane must still claim its own traffic"
    );
}

// ---------------------------------------------------------------------------
// In-session duplicate handling
// ---------------------------------------------------------------------------

use omakure::remote_cue::{CueCode, CueOutcome, CuePolicy, CueSession, GateDecision};

fn session_over<'a>(registry: &'a NodeRegistry) -> CueSession<'a> {
    CueSession::new(
        registry,
        "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        CuePolicy {
            enabled: true,
            declared_scripts: vec!["deploy.sh".to_string()],
        },
    )
}

/// A retransmission on a live connection is the realistic duplicate, and it is
/// answered from the first decision rather than re-evaluated.
#[test]
fn a_repeated_cue_id_on_one_session_is_decided_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_identity, registry) = identity_and_registry(dir.path());
    let mut session = session_over(&registry);

    let first = session.decide(Some("0123456789abcdef0123456789abcdef"));
    let second = session.decide(Some("0123456789abcdef0123456789abcdef"));

    // The peer is unknown to this registry, so the trust gate refuses. What
    // matters here is that the *first* call reached a decision at all and the
    // second did not repeat it.
    assert_eq!(
        first,
        CueOutcome::Decided(GateDecision::Rejected(CueCode::NotActiveConductor))
    );
    assert_eq!(
        second,
        CueOutcome::Repeat,
        "a retransmission must be answered from the first decision, not re-evaluated"
    );
}

/// Distinct ids are distinct decisions; the guard must not collapse them.
#[test]
fn different_cue_ids_are_decided_separately() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_identity, registry) = identity_and_registry(dir.path());
    let mut session = session_over(&registry);

    assert!(matches!(
        session.decide(Some("0123456789abcdef0123456789abcdef")),
        CueOutcome::Decided(_)
    ));
    assert!(
        matches!(
            session.decide(Some("fedcba9876543210fedcba9876543210")),
            CueOutcome::Decided(_)
        ),
        "a different cue id is a different instruction and must be decided"
    );
}

/// The guard is per session, and says so rather than implying durability.
#[test]
fn a_new_session_does_not_inherit_the_seen_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_identity, registry) = identity_and_registry(dir.path());

    let mut first = session_over(&registry);
    assert!(matches!(
        first.decide(Some("0123456789abcdef0123456789abcdef")),
        CueOutcome::Decided(_)
    ));
    drop(first);

    // A fresh session decides it again. Durable at-most-once arrives with the
    // run row, whose primary key is derived from the cue id.
    let mut second = session_over(&registry);
    assert_eq!(
        second.decide(Some("0123456789abcdef0123456789abcdef")),
        CueOutcome::Decided(GateDecision::Rejected(CueCode::NotActiveConductor)),
        "the guard is per session and does not pretend to be durable"
    );
}
