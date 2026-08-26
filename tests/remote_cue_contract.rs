//! Executable Remote Cue contract.
//!
//! The machine-checked half of `.docs/remote-cue-contract.md`. It pins every
//! frozen bound, builds a canonical reference vector for each of the two message
//! kinds, verifies them through the production `omakure::direct_transport`
//! envelope path, and asserts the properties that make remote execution safe
//! rather than merely possible.
//!
//! No production Cue surface exists yet. Messages are constructed here with the
//! frozen direct-envelope construction, which proves the plane is carriable
//! without any change to the frozen identity construction or to transport code.
//!
//! Several assertions below pin *shipped behaviour this contract must work
//! against* rather than behaviour the contract introduces — the allow-all secret
//! default and the lease-steal window in particular. They are here so that a
//! future change to either one breaks this test instead of silently widening
//! what a remote caller can reach.

use k256::schnorr::{signature::hazmat::PrehashSigner, SigningKey};
use omakure::direct_transport::{envelope_nonce, verify_envelope};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const ENVELOPE_DOMAIN: &[u8] = b"omakure/direct-envelope/v1\0";
const RUN_ID_DOMAIN: &[u8] = b"omakure/cue-run-id/v1\0";
const CONTRACT_ID: &str = "omakure/remote-cue/v1";
const CUE_VERSION: u64 = 1;

/// Published test scalar 1 from `tests/fixtures/node_identity_vectors.toml`.
const CONDUCTOR_SCALAR_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2), "hex length must be even");
    (0..value.len() / 2)
        .map(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).expect("hex byte"))
        .collect()
}

fn signing_key() -> SigningKey {
    SigningKey::from_slice(&unhex(CONDUCTOR_SCALAR_HEX)).expect("test scalar")
}

fn x_only_public_key() -> [u8; 32] {
    signing_key()
        .verifying_key()
        .to_bytes()
        .as_slice()
        .try_into()
        .expect("x-only key length")
}

/// The frozen node-id derivation, mirroring the sibling Health Plane contract
/// test. No production surface is added.
fn conductor_node_id() -> String {
    let mut input = b"omakure/node-id/v1\0".to_vec();
    input.extend_from_slice(&x_only_public_key());
    format!("omk1_{}", hex(Sha256::digest(input).as_slice()))
}

fn vectors() -> toml::Value {
    let raw = std::fs::read_to_string("tests/fixtures/remote_cue_vectors.toml")
        .expect("read the frozen Cue vectors");
    toml::from_str(&raw).expect("parse the frozen Cue vectors")
}

fn int(v: &toml::Value, path: &[&str]) -> i64 {
    let mut cur = v;
    for key in path {
        cur = cur.get(key).unwrap_or_else(|| panic!("missing {path:?}"));
    }
    cur.as_integer()
        .unwrap_or_else(|| panic!("{path:?} is not an integer"))
}

fn boolean(v: &toml::Value, path: &[&str]) -> bool {
    let mut cur = v;
    for key in path {
        cur = cur.get(key).unwrap_or_else(|| panic!("missing {path:?}"));
    }
    cur.as_bool()
        .unwrap_or_else(|| panic!("{path:?} is not a boolean"))
}

fn text<'a>(v: &'a toml::Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for key in path {
        cur = cur.get(key).unwrap_or_else(|| panic!("missing {path:?}"));
    }
    cur.as_str()
        .unwrap_or_else(|| panic!("{path:?} is not a string"))
}

// ---------------------------------------------------------------------------
// Identity of the plane
// ---------------------------------------------------------------------------

#[test]
fn the_contract_identifies_itself_and_changes_no_frozen_construction() {
    let v = vectors();
    assert_eq!(text(&v, &["contract_id"]), CONTRACT_ID);
    assert_eq!(
        text(&v, &["contract_document"]),
        ".docs/remote-cue-contract.md"
    );
    assert_eq!(text(&v, &["status"]), "frozen-pending-owner-review");
    assert_eq!(int(&v, &["cue_version"]), CUE_VERSION as i64);
    assert_eq!(text(&v, &["signing_algorithm"]), "BIP-340-Schnorr");
    assert_eq!(text(&v, &["canonicalization"]), "RFC-8785");

    // The whole safety argument rests on reusing constructions that were already
    // frozen and certified. If either of these ever goes true, this contract is
    // no longer describing the system it was reviewed against.
    assert!(!boolean(&v, &["identity_construction_changed"]));
    assert!(!boolean(&v, &["transport_code_changed"]));

    assert_eq!(
        hex(ENVELOPE_DOMAIN),
        text(&v, &["envelope_signature_domain_hex"])
    );
    assert_eq!(
        hex(RUN_ID_DOMAIN),
        text(&v, &["run_id_domain_hex"]),
        "the run-id derivation must have its own domain separator so a cue_id \
         can never be replayed as a preimage in another construction"
    );
}

#[test]
fn the_cue_plane_is_a_sibling_and_leaves_the_closed_sets_closed() {
    let v = vectors();
    assert_eq!(text(&v, &["kind_prefix"]), "cue_");
    assert_eq!(text(&v, &["health_kind_prefix"]), "health_");
    assert_eq!(
        int(&v, &["health_kinds_remain"]),
        omakure::health_plane::model::HealthKind::ALL.len() as i64,
        "HealthKind must stay closed; a Cue is not a sixth Health kind"
    );
    assert_eq!(int(&v, &["signal_kinds_remain"]), 3);
    assert!(!boolean(&v, &["signal_field_added"]));
}

#[test]
fn there_are_exactly_two_kinds_and_the_outcome_is_not_one_of_them() {
    let v = vectors();
    let kinds: Vec<&str> = v["kinds"]
        .as_array()
        .expect("kinds array")
        .iter()
        .map(|k| k.as_str().expect("kind string"))
        .collect();
    assert_eq!(kinds, vec!["cue_dispatch", "cue_ack"]);
    for kind in &kinds {
        assert!(kind.starts_with("cue_"));
        assert!(kind.len() as i64 <= int(&v, &["sizes", "max_kind_bytes"]));
    }

    // The terminal result rides the existing Signal. This is why gate D exists:
    // a peer that could never deliver an outcome must not be able to create work.
    assert_eq!(
        text(&v, &["outcome_carrier"]),
        "health_signal:run-completed"
    );
}

// ---------------------------------------------------------------------------
// Authorization
// ---------------------------------------------------------------------------

#[test]
fn all_four_gates_are_frozen_and_read_only_from_the_local_registry() {
    let v = vectors();
    let gates: Vec<&str> = v["authorization"]["gates"]
        .as_array()
        .expect("gates array")
        .iter()
        .map(|g| g.as_str().expect("gate string"))
        .collect();
    assert_eq!(
        gates,
        vec![
            "allow_remote_cues",
            "active_conductor_role",
            "remote_run_capability",
            "notifications_capability"
        ]
    );

    // The single most important property in this document.
    assert!(
        !boolean(&v, &["authorization", "reads_authorization_from_message"]),
        "no inbound field may contribute to the decision to accept it"
    );

    // Freeze what ships, not the TEXT role sketched in the rebuild notes.
    assert_eq!(text(&v, &["authorization", "role_encoding"]), "integer");
    assert_eq!(
        int(&v, &["authorization", "role_conductor"]),
        omakure::health_plane::bounds::ROLE_CONDUCTOR
    );
    assert_eq!(
        int(&v, &["authorization", "role_performer"]),
        omakure::health_plane::bounds::ROLE_PERFORMER
    );
}

/// Both required capabilities already ship, so this phase amends no allow-list.
#[test]
fn the_required_capabilities_already_exist_in_the_frozen_allowlist() {
    let v = vectors();
    assert!(!boolean(
        &v,
        &["authorization", "capabilities_allowlist_amended"]
    ));
    let allowlist: HashSet<&str> = omakure::health_plane::bounds::CAPABILITY_ALLOWLIST
        .iter()
        .copied()
        .collect();
    for required in v["authorization"]["required_capabilities"]
        .as_array()
        .expect("required capabilities")
    {
        let name = required.as_str().expect("capability string");
        assert!(
            allowlist.contains(name),
            "{name} must already be in the frozen capability allow-list"
        );
    }
}

// ---------------------------------------------------------------------------
// The dangerous defaults this contract must work against
// ---------------------------------------------------------------------------

/// Pins the shipped allow-all secret default so a Cue can never inherit it.
///
/// `None` writes ALLOW_ALL, and a policy *lookup error* also grants allow-all.
/// A Cue-origin run must therefore always be written with an explicit empty
/// policy. If this assertion ever fails because the default changed, revisit the
/// contract rule rather than deleting the test.
#[test]
fn a_cue_run_must_carry_an_explicit_deny_all_secret_policy() {
    let v = vectors();
    assert!(
        boolean(&v, &["secrets", "none_means_allow_all"]),
        "the shipped default is allow-all; the Cue rule exists because of it"
    );
    assert_eq!(
        text(&v, &["secrets", "cue_run_policy"]),
        "explicit-empty-deny-all"
    );
    assert!(boolean(
        &v,
        &["secrets", "secret_bearing_script_is_rejected"]
    ));
}

/// Pins the lease-steal window that would otherwise break at-most-once.
#[test]
fn a_cue_run_is_never_lease_stolen() {
    let v = vectors();
    assert_eq!(
        int(&v, &["at_most_once", "heartbeat_ms"]),
        omakure::HEARTBEAT_MS,
        "the frozen figure must track the shipped lease window"
    );
    assert!(
        !boolean(&v, &["at_most_once", "cue_runs_are_lease_stealable"]),
        "re-claiming a crashed Cue run converts at-most-once into at-least-once"
    );
    assert_eq!(
        text(&v, &["at_most_once", "expired_lease_terminal_state"]),
        "failed"
    );
}

/// `RunTrigger::Cue` is the discriminator that makes both of the above possible,
/// and without it the Health Plane reports a remote run as `manual`.
#[test]
fn cue_provenance_is_representable_and_distinct() {
    assert_eq!(omakure::RunTrigger::Cue.as_str(), "Cue");
    assert_ne!(omakure::RunTrigger::Cue, omakure::RunTrigger::Manual);
    assert_ne!(omakure::RunTrigger::Cue, omakure::RunTrigger::Scheduled);
}

// ---------------------------------------------------------------------------
// Resolution, idempotency, liveness, bounds
// ---------------------------------------------------------------------------

#[test]
fn a_cue_names_a_script_and_never_carries_one() {
    let v = vectors();
    assert!(
        !boolean(&v, &["resolution", "carries_script_content"]),
        "remote management must not be able to introduce code onto a node"
    );
    assert!(boolean(&v, &["resolution", "requires_regular_file"]));
    assert!(boolean(&v, &["resolution", "hash_recorded_at_accept"]));
    assert!(
        boolean(&v, &["resolution", "hash_reverified_at_exec"]),
        "without re-verification a file swapped between accept and exec runs instead"
    );
    assert!(
        boolean(&v, &["resolution", "missing_and_excluded_share_a_code"]),
        "distinguishing them turns a Cue into a workspace enumeration oracle"
    );
}

#[test]
fn idempotency_rests_on_the_database_primary_key() {
    let v = vectors();
    assert!(boolean(
        &v,
        &["idempotency", "run_id_is_derived_from_cue_id"]
    ));
    assert_eq!(
        text(&v, &["idempotency", "durable_key"]),
        "runs.run_id TEXT PRIMARY KEY"
    );
    assert!(boolean(
        &v,
        &["idempotency", "conductor_computes_expected_opaque_run_id"]
    ));
}

#[test]
fn liveness_rules_close_the_revocation_windows() {
    let v = vectors();
    for rule in [
        "gates_reevaluated_in_accept_transaction",
        "revocation_cancels_in_flight_run",
        "pre_revocation_cue_ids_permanently_rejected",
        "expiry_checked_at_both_transitions",
    ] {
        assert!(boolean(&v, &["liveness", rule]), "{rule} must hold");
    }
}

#[test]
fn every_bound_is_frozen_to_an_exact_number() {
    let v = vectors();
    assert_eq!(int(&v, &["bounds", "concurrent_cue_runs_per_peer"]), 1);
    assert_eq!(int(&v, &["bounds", "cues_per_peer_per_minute"]), 10);
    assert_eq!(
        int(&v, &["bounds", "rate_burst_allowance"]),
        omakure::health_plane::bounds::RATE_BURST_ALLOWANCE
    );
    assert_eq!(
        int(&v, &["bounds", "retained_cue_records_per_peer"]),
        omakure::health_plane::bounds::SIGNAL_INBOX_CAPACITY
    );
    assert_eq!(
        int(&v, &["bounds", "cue_retention_seconds"]),
        omakure::health_plane::bounds::SIGNAL_RETENTION_SECONDS
    );
    assert_eq!(int(&v, &["bounds", "max_lifetime_seconds"]), 300);

    assert_eq!(
        int(&v, &["grammar", "max_script_bytes"]),
        omakure::health_plane::bounds::MAX_SCRIPT_BYTES as i64
    );
    assert_eq!(
        int(&v, &["grammar", "cue_id_hex_chars"]),
        omakure::health_plane::bounds::OPAQUE_ID_HEX_CHARS as i64
    );
}

/// The band must not collide with transport or Health codes.
#[test]
fn error_codes_are_unique_and_in_a_disjoint_band() {
    let v = vectors();
    let codes: Vec<i64> = v["error_code"]
        .as_array()
        .expect("error_code array")
        .iter()
        .map(|e| e["code"].as_integer().expect("code integer"))
        .collect();

    assert_eq!(codes.len(), 11, "every documented code must be vectored");
    let unique: HashSet<i64> = codes.iter().copied().collect();
    assert_eq!(unique.len(), codes.len(), "codes must be unique");

    for code in codes {
        assert!(
            (1201..=1299).contains(&code),
            "{code} is outside the Cue band"
        );
        assert!(
            !(1001..=1020).contains(&code),
            "{code} collides with transport codes"
        );
        assert!(
            !(1101..=1115).contains(&code),
            "{code} collides with Health Plane codes"
        );
        assert!(
            (1000..=1999).contains(&code),
            "{code} is outside the transport_audit error_code range"
        );
    }
}

// ---------------------------------------------------------------------------
// Canonical reference vectors, verified through the production envelope path
// ---------------------------------------------------------------------------

/// Build one canonical envelope exactly as the frozen construction requires.
fn signed(kind: &str, payload: Value, session_id: &[u8; 32], nonce: [u8; 16], now: u64) -> Vec<u8> {
    let envelope = json!({
        "created_at": now,
        "kind": kind,
        "nonce": hex(&nonce),
        "payload": payload,
        "sender": conductor_node_id(),
        "session_id": hex(session_id),
        "version": 1,
    });
    let canonical = serde_jcs::to_vec(&envelope).expect("canonicalize");
    let mut hasher = Sha256::new();
    hasher.update(ENVELOPE_DOMAIN);
    hasher.update(&canonical);
    let digest = hasher.finalize();
    let signature: k256::schnorr::Signature = signing_key().sign_prehash(&digest).expect("sign");
    let mut encoded = canonical;
    encoded.extend_from_slice(&signature.to_bytes());
    encoded
}

#[test]
fn a_reference_cue_dispatch_verifies_through_the_shipped_envelope_path() {
    let v = vectors();
    let session_id = [7u8; 32];
    let nonce = [9u8; 16];
    let now = 1_800_000_000u64;

    let payload = json!({
        "version": CUE_VERSION,
        "cue_id": "0123456789abcdef0123456789abcdef",
        "script": "deploy.sh",
        "not_before": now,
        "expires_at": now + 300,
        "reason": "contract reference vector",
    });
    let encoded = signed("cue_dispatch", payload, &session_id, nonce, now);

    assert!(
        encoded.len() as i64 <= int(&v, &["sizes", "max_canonical_cue_dispatch"]) + 64,
        "the reference vector must fit the frozen size bound plus its signature"
    );

    let read_nonce = envelope_nonce(&encoded).expect("read the nonce back");
    verify_envelope(
        &encoded,
        &conductor_node_id(),
        &x_only_public_key(),
        "cue_dispatch",
        &session_id,
        &read_nonce,
    )
    .expect("a cue_dispatch must verify under the frozen construction");
}

#[test]
fn a_reference_cue_ack_verifies_through_the_shipped_envelope_path() {
    let session_id = [7u8; 32];
    let nonce = [11u8; 16];
    let now = 1_800_000_000u64;

    let payload = json!({
        "version": CUE_VERSION,
        "cue_id": "0123456789abcdef0123456789abcdef",
        "accepted": false,
        "error": {"code": 1201},
    });
    let encoded = signed("cue_ack", payload, &session_id, nonce, now);

    let read_nonce = envelope_nonce(&encoded).expect("read the nonce back");
    verify_envelope(
        &encoded,
        &conductor_node_id(),
        &x_only_public_key(),
        "cue_ack",
        &session_id,
        &read_nonce,
    )
    .expect("a cue_ack must verify under the frozen construction");
}

/// An envelope signed for one kind must not verify as another.
#[test]
fn a_dispatch_cannot_be_replayed_as_an_ack() {
    let session_id = [7u8; 32];
    let nonce = [9u8; 16];
    let now = 1_800_000_000u64;
    let encoded = signed(
        "cue_dispatch",
        json!({"version": CUE_VERSION}),
        &session_id,
        nonce,
        now,
    );
    let read_nonce = envelope_nonce(&encoded).expect("nonce");

    let result = verify_envelope(
        &encoded,
        &conductor_node_id(),
        &x_only_public_key(),
        "cue_ack",
        &session_id,
        &read_nonce,
    );
    assert!(result.is_err(), "kind must be bound by the signature");
}

/// A dispatch bound to one session must not verify inside another.
#[test]
fn a_dispatch_cannot_be_replayed_into_another_session() {
    let nonce = [9u8; 16];
    let now = 1_800_000_000u64;
    let encoded = signed(
        "cue_dispatch",
        json!({"version": CUE_VERSION}),
        &[7u8; 32],
        nonce,
        now,
    );
    let read_nonce = envelope_nonce(&encoded).expect("nonce");

    let result = verify_envelope(
        &encoded,
        &conductor_node_id(),
        &x_only_public_key(),
        "cue_dispatch",
        &[8u8; 32],
        &read_nonce,
    );
    assert!(result.is_err(), "session must be bound by the signature");
}
