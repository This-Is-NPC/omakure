//! Executable Health Plane contract.
//!
//! This test is the machine-checked half of `docs/internal/health-plane-contract.md`.
//! It pins every frozen bound, builds the canonical reference vectors for each
//! of the five message kinds, verifies them through the production
//! `omakure::direct_transport` envelope path, and drives a reference receiver
//! that must reject every contracted adversarial case with its stable error
//! code.
//!
//! No production Health Plane surface exists yet. Messages are constructed here
//! with the frozen direct-envelope construction (RFC-8785 canonical JSON plus a
//! BIP-340 signature over the frozen domain) and verified with the shipped
//! `verify_envelope`, which proves the Health Plane is carriable without any
//! change to the frozen identity construction.

use k256::schnorr::{signature::hazmat::PrehashSigner, SigningKey};
use omakure::direct_transport::{envelope_nonce, verify_envelope, TransportError};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

const ENVELOPE_DOMAIN: &[u8] = b"omakure/direct-envelope/v1\0";
const CONTRACT_ID: &str = "omakure/health-plane/v1";
const HEALTH_VERSION: u64 = 1;
const REGISTRY_SCHEMA_VERSION: i64 = 8;

/// Published test scalar 1 from `tests/fixtures/node_identity_vectors.toml`.
const PERFORMER_SCALAR_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000001";
/// Published test scalar 2 from `tests/fixtures/node_identity_vectors.toml`.
const CONDUCTOR_SCALAR_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000002";
const SESSION_ID_HEX: &str = "3131313131313131313131313131313131313131313131313131313131313131";
const OTHER_SESSION_ID_HEX: &str =
    "3232323232323232323232323232323232323232323232323232323232323232";
const BASE_NOW: u64 = 1_700_000_000;

// ---------------------------------------------------------------------------
// Frozen bounds
// ---------------------------------------------------------------------------

const ROLE_CONDUCTOR: u8 = 1;
const ROLE_PERFORMER: u8 = 2;

const CAPABILITY_ALLOWLIST: [&str; 7] = [
    "backup-orchestration",
    "baseline-push",
    "inventory-health",
    "lost-device-revocation",
    "notifications",
    "remote-run",
    "ssh-credential-rotation",
];
const CAPABILITY_PROFILE_PULSE: &str = "inventory-health";
const CAPABILITY_SIGNAL: &str = "notifications";

const RUNTIME_NAMES: [&str; 4] = ["bash", "powershell", "python", "sh"];

const MAX_CANONICAL_PROFILE: usize = 2_048;
const MAX_CANONICAL_PULSE: usize = 1_280;
const MAX_CANONICAL_SIGNAL: usize = 1_024;
const MAX_CANONICAL_ACK: usize = 768;
const MAX_CANONICAL_ERROR: usize = 768;
const SIGNATURE_BYTES: usize = 64;

const MAX_JSON_DEPTH: usize = 5;
const MAX_FIELD_NAME_BYTES: usize = 32;
const MAX_PAYLOAD_FIELDS: usize = 64;
const MAX_ARRAY_LENGTH: usize = 32;
const MAX_STRING_BYTES: usize = 128;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const NODE_ID_BYTES: usize = 69;
const HEX16_CHARS: usize = 32;

const MAX_AGENT_VERSION_BYTES: usize = 32;
const BASELINE_ID_HEX_CHARS: usize = 64;
/// The reference Profile reports the same set recorded and observed, which is
/// the in-sync case; drift is the two differing and needs no second vector.
const REFERENCE_BASELINE_ID: &str =
    "3f0a91c4d2b85e67a1c30f4e8b29d75641aeb0c3928f5d61b7e04a2c8d9f1350";
const MAX_DISPLAY_NAME_BYTES: usize = 64;
const MAX_DISTRO_ID_BYTES: usize = 32;
const MAX_DISTRO_VERSION_BYTES: usize = 32;
const MAX_SCRIPT_BYTES: usize = 64;
const MAX_CAPABILITY_BYTES: usize = 64;
const MAX_CAPABILITY_COUNT: usize = 32;
const MAX_RUNTIME_COUNT: usize = 4;
const MAX_WORKERS: u64 = 255;
const MAX_QUEUE_DEPTH: u64 = 65_535;
const MAX_UPTIME_SECONDS: u64 = 4_294_967_295;
const MIN_EXIT_CODE: i64 = -256;
const MAX_EXIT_CODE: i64 = 255;

const MAX_AGE_SECONDS: u64 = 120;
const MAX_FUTURE_SKEW_SECONDS: u64 = 60;
const PRESENCE_ONLINE_SECONDS: u64 = 90;
const PRESENCE_STALE_SECONDS: u64 = 600;

const NOMINAL_PULSE_INTERVAL_SECONDS: u64 = 30;
const MIN_PULSE_INTERVAL_SECONDS: u64 = 10;
const MAX_MESSAGES_PER_PEER_PER_MINUTE: u32 = 20;
const MAX_PROFILES_PER_PEER_PER_HOUR: u32 = 12;
const MAX_SIGNALS_PER_PEER_PER_MINUTE: u32 = 10;
const RATE_BURST_ALLOWANCE: u32 = 5;
const MAX_IN_FLIGHT_PER_SESSION: u32 = 8;

const MAX_PERFORMERS_PER_CONDUCTOR: usize = 256;
const MAX_CONDUCTORS_PER_PERFORMER: usize = 1;

const SIGNAL_OUTBOX_CAPACITY: usize = 64;
const SIGNAL_INBOX_CAPACITY: usize = 64;
const SIGNAL_GLOBAL_INBOX_CAPACITY: usize = 16_384;
const ACK_TIMEOUT_SECONDS: u64 = 5;
const MAX_RETRIES: usize = 3;
const RETRY_BACKOFF_SECONDS: [u64; 3] = [1, 2, 4];
const PROCESSING_BUDGET_MILLIS: u64 = 250;

const REORDER_BUFFER_ENTRIES: u64 = 32;
const REORDER_BUFFER_SECONDS: u64 = 60;

const REPLAY_SECURITY_FLOOR_SECONDS: u64 = 180;
const REPLAY_RETENTION_SECONDS: u64 = 900;
const MAX_REPLAY_ROWS: u64 = 131_072;
const REPLAY_ROW_BYTES: u64 = 32;

const SIGNAL_RETENTION_SECONDS: u64 = 604_800;
const MAX_STORED_PROFILE_BYTES: u64 = 2_112;
const MAX_STORED_PULSE_BYTES: u64 = 1_344;
const MAX_STORED_SIGNAL_BYTES: u64 = 1_088;
const MAX_AUDIT_ROWS: u64 = 10_000;
const AUDIT_ROW_BYTES: u64 = 256;
const AUDIT_RETENTION_SECONDS: u64 = 2_592_000;
const STORAGE_CEILING_BYTES: u64 = 25_464_832;

const VERSION_INCOMPATIBLE_BACKOFF_SECONDS: u64 = 300;
const VERSION_INCOMPATIBLE_EXPIRY_SECONDS: u64 = 3_600;

// ---------------------------------------------------------------------------
// Stable error codes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthCode {
    UnsupportedVersion = 1101,
    InvalidMessage = 1102,
    MessageTooLarge = 1103,
    WrongTarget = 1104,
    WrongRole = 1105,
    MissingCapability = 1106,
    Revoked = 1107,
    Stale = 1108,
    Future = 1109,
    Replay = 1110,
    Reordered = 1111,
    RateLimited = 1112,
    QueueFull = 1113,
    UnknownField = 1114,
    CorruptState = 1115,
}

impl HealthCode {
    const ALL: [HealthCode; 15] = [
        HealthCode::UnsupportedVersion,
        HealthCode::InvalidMessage,
        HealthCode::MessageTooLarge,
        HealthCode::WrongTarget,
        HealthCode::WrongRole,
        HealthCode::MissingCapability,
        HealthCode::Revoked,
        HealthCode::Stale,
        HealthCode::Future,
        HealthCode::Replay,
        HealthCode::Reordered,
        HealthCode::RateLimited,
        HealthCode::QueueFull,
        HealthCode::UnknownField,
        HealthCode::CorruptState,
    ];

    const fn code(self) -> u16 {
        self as u16
    }

    const fn name(self) -> &'static str {
        match self {
            HealthCode::UnsupportedVersion => "health_unsupported_version",
            HealthCode::InvalidMessage => "health_invalid_message",
            HealthCode::MessageTooLarge => "health_message_too_large",
            HealthCode::WrongTarget => "health_wrong_target",
            HealthCode::WrongRole => "health_wrong_role",
            HealthCode::MissingCapability => "health_missing_capability",
            HealthCode::Revoked => "health_revoked",
            HealthCode::Stale => "health_stale",
            HealthCode::Future => "health_future",
            HealthCode::Replay => "health_replay",
            HealthCode::Reordered => "health_reordered",
            HealthCode::RateLimited => "health_rate_limited",
            HealthCode::QueueFull => "health_queue_full",
            HealthCode::UnknownField => "health_unknown_field",
            HealthCode::CorruptState => "health_corrupt_state",
        }
    }

    fn from_transport(error: TransportError) -> Self {
        match error {
            TransportError::Replay => HealthCode::Replay,
            TransportError::MessageTooLarge => HealthCode::MessageTooLarge,
            _ => HealthCode::InvalidMessage,
        }
    }
}

// ---------------------------------------------------------------------------
// Message kinds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Profile,
    Pulse,
    Signal,
    Ack,
    Error,
}

impl Kind {
    const ALL: [Kind; 5] = [
        Kind::Profile,
        Kind::Pulse,
        Kind::Signal,
        Kind::Ack,
        Kind::Error,
    ];

    const fn wire(self) -> &'static str {
        match self {
            Kind::Profile => "health_profile",
            Kind::Pulse => "health_pulse",
            Kind::Signal => "health_signal",
            Kind::Ack => "health_ack",
            Kind::Error => "health_error",
        }
    }

    const fn body_field(self) -> &'static str {
        match self {
            Kind::Profile => "profile",
            Kind::Pulse => "pulse",
            Kind::Signal => "signal",
            Kind::Ack => "ack",
            Kind::Error => "error",
        }
    }

    const fn max_canonical_bytes(self) -> usize {
        match self {
            Kind::Profile => MAX_CANONICAL_PROFILE,
            Kind::Pulse => MAX_CANONICAL_PULSE,
            Kind::Signal => MAX_CANONICAL_SIGNAL,
            Kind::Ack => MAX_CANONICAL_ACK,
            Kind::Error => MAX_CANONICAL_ERROR,
        }
    }

    const fn required_role(self) -> u8 {
        match self {
            Kind::Profile | Kind::Pulse | Kind::Signal => ROLE_PERFORMER,
            Kind::Ack | Kind::Error => ROLE_CONDUCTOR,
        }
    }

    const fn required_capability(self) -> Option<&'static str> {
        match self {
            Kind::Profile | Kind::Pulse => Some(CAPABILITY_PROFILE_PULSE),
            Kind::Signal => Some(CAPABILITY_SIGNAL),
            Kind::Ack | Kind::Error => None,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Kind::ALL.into_iter().find(|kind| kind.wire() == value)
    }
}

// ---------------------------------------------------------------------------
// Frozen construction helpers
// ---------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2), "hex length must be even");
    (0..value.len() / 2)
        .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).expect("hex digit"))
        .collect()
}

fn canonical(value: &Value) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("canonical JSON")
}

fn signing_key(scalar_hex: &str) -> SigningKey {
    SigningKey::from_slice(&unhex(scalar_hex)).expect("test scalar")
}

fn x_only_public_key(scalar_hex: &str) -> [u8; 32] {
    signing_key(scalar_hex)
        .verifying_key()
        .to_bytes()
        .as_slice()
        .try_into()
        .expect("x-only key length")
}

fn node_id(scalar_hex: &str) -> String {
    let mut input = b"omakure/node-id/v1\0".to_vec();
    input.extend_from_slice(&x_only_public_key(scalar_hex));
    format!("omk1_{}", hex(Sha256::digest(input).as_slice()))
}

/// Build the frozen seven-field direct envelope and sign it with the frozen
/// BIP-340 construction. This mirrors the private `sign_envelope` in
/// `src/direct_transport.rs` byte for byte and adds no production surface.
fn sign_envelope(
    scalar_hex: &str,
    kind: &str,
    session_id_hex: &str,
    nonce_hex: &str,
    payload: Value,
    created_at: u64,
) -> (Vec<u8>, Vec<u8>) {
    let envelope = json!({
        "created_at": created_at,
        "kind": kind,
        "nonce": nonce_hex,
        "payload": payload,
        "sender": node_id(scalar_hex),
        "session_id": session_id_hex,
        "version": 1,
    });
    let canonical_bytes = canonical(&envelope);
    let digest = Sha256::digest([ENVELOPE_DOMAIN, canonical_bytes.as_slice()].concat());
    let signature = signing_key(scalar_hex)
        .sign_prehash(&digest)
        .expect("sign prehash")
        .to_bytes()
        .to_vec();
    (canonical_bytes, signature)
}

fn encode(canonical_bytes: &[u8], signature: &[u8]) -> Vec<u8> {
    let mut encoded = canonical_bytes.to_vec();
    encoded.extend_from_slice(signature);
    encoded
}

// ---------------------------------------------------------------------------
// Reference message builders
// ---------------------------------------------------------------------------

fn nonce_hex(seed: u8) -> String {
    hex(&[seed; 16])
}

fn message_id(seed: u8) -> String {
    hex(&[seed; 16])
}

fn performer_id() -> String {
    node_id(PERFORMER_SCALAR_HEX)
}

fn conductor_id() -> String {
    node_id(CONDUCTOR_SCALAR_HEX)
}

fn profile_payload(target: &str, revision: u64) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id(0x11),
        "target": target,
        "profile": {
            "agent_version": "0.3.0",
            "arch": "x86_64",
            "baseline_id": REFERENCE_BASELINE_ID,
            "baseline_observed_id": REFERENCE_BASELINE_ID,
            "capabilities": [CAPABILITY_PROFILE_PULSE, CAPABILITY_SIGNAL],
            "display_name": "workshop-laptop",
            "distro_id": "arch",
            "distro_version": "rolling",
            "omarchy_channel": "stable",
            "omarchy_version": "2.1.0",
            "platform": "linux",
            "profile_revision": revision,
            "role": "performer",
            "runtimes": [
                {"available": true, "name": "bash", "version": "5.2.37"},
                {"available": false, "name": "powershell", "version": ""},
                {"available": true, "name": "python", "version": "3.13.1"},
                {"available": true, "name": "sh", "version": "5.2.37"}
            ]
        }
    })
}

fn pulse_payload(target: &str, sequence: u64, emitted_at: u64) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id(0x22),
        "target": target,
        "pulse": {
            "emitted_at": emitted_at,
            "last_run": {
                "exit_code": 0,
                "finished_at": 1_699_999_990_u64,
                "run_id": message_id(0xa1),
                "script": "deploy",
                "started_at": 1_699_999_980_u64,
                "state": "completed",
                "trigger": "scheduled"
            },
            "profile_revision": 1,
            "runner": {
                "queue_depth": 0,
                "scheduler": "running",
                "state": "idle",
                "workers_busy": 0,
                "workers_configured": 1
            },
            "sequence": sequence,
            "uptime_seconds": 3600
        }
    })
}

fn signal_payload(target: &str, sequence: u64, signal_seed: u8) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id(0x33),
        "target": target,
        "signal": {
            "kind": "run-completed",
            "occurred_at": 1_699_999_990_u64,
            "run": {
                "exit_code": 0,
                "finished_at": 1_699_999_990_u64,
                "run_id": message_id(0xa1),
                "script": "deploy",
                "state": "completed"
            },
            "sequence": sequence,
            "signal_id": message_id(signal_seed),
            "subject": Value::Null
        }
    })
}

fn ack_payload(target: &str, cursor: u64) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id(0x44),
        "target": target,
        "ack": {
            "accepted": true,
            "acked_message_id": message_id(0x33),
            "cursor": cursor
        }
    })
}

fn error_payload(target: &str, code: HealthCode) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id(0x55),
        "target": target,
        "error": {
            "accepted": false,
            "acked_message_id": message_id(0x33),
            "code": code.code(),
            "reason": code.name()
        }
    })
}

fn reference_payload(kind: Kind) -> Value {
    match kind {
        Kind::Profile => profile_payload(&conductor_id(), 1),
        Kind::Pulse => pulse_payload(&conductor_id(), 1, BASE_NOW),
        Kind::Signal => signal_payload(&conductor_id(), 1, 0xb1),
        Kind::Ack => ack_payload(&performer_id(), 1),
        Kind::Error => error_payload(&performer_id(), HealthCode::Replay),
    }
}

fn reference_scalar(kind: Kind) -> &'static str {
    match kind {
        Kind::Profile | Kind::Pulse | Kind::Signal => PERFORMER_SCALAR_HEX,
        Kind::Ack | Kind::Error => CONDUCTOR_SCALAR_HEX,
    }
}

fn reference_message(kind: Kind) -> Vec<u8> {
    let (canonical_bytes, signature) = sign_envelope(
        reference_scalar(kind),
        kind.wire(),
        SESSION_ID_HEX,
        &nonce_hex(0x01),
        reference_payload(kind),
        BASE_NOW,
    );
    encode(&canonical_bytes, &signature)
}

/// Every bounded string, array, and integer at its frozen maximum.
fn worst_case_payload(kind: Kind) -> Value {
    let target = conductor_id();
    match kind {
        Kind::Profile => json!({
            "health_version": HEALTH_VERSION,
            "message_id": message_id(0x11),
            "target": target,
            "profile": {
                "agent_version": "9".repeat(MAX_AGENT_VERSION_BYTES),
                "arch": "x86_64",
                "baseline_id": "b".repeat(BASELINE_ID_HEX_CHARS),
                "baseline_observed_id": "c".repeat(BASELINE_ID_HEX_CHARS),
                "capabilities": CAPABILITY_ALLOWLIST,
                "display_name": "d".repeat(MAX_DISPLAY_NAME_BYTES),
                "distro_id": "d".repeat(MAX_DISTRO_ID_BYTES),
                "distro_version": "v".repeat(MAX_DISTRO_VERSION_BYTES),
                "omarchy_channel": "stable",
                "omarchy_version": "v".repeat(MAX_DISTRO_VERSION_BYTES),
                "platform": "windows",
                "profile_revision": MAX_SAFE_INTEGER,
                "role": "performer",
                "runtimes": [
                    {"available": true, "name": "bash", "version": "v".repeat(MAX_DISTRO_VERSION_BYTES)},
                    {"available": true, "name": "powershell", "version": "v".repeat(MAX_DISTRO_VERSION_BYTES)},
                    {"available": true, "name": "python", "version": "v".repeat(MAX_DISTRO_VERSION_BYTES)},
                    {"available": true, "name": "sh", "version": "v".repeat(MAX_DISTRO_VERSION_BYTES)}
                ]
            }
        }),
        Kind::Pulse => json!({
            "health_version": HEALTH_VERSION,
            "message_id": message_id(0x22),
            "target": target,
            "pulse": {
                "emitted_at": MAX_SAFE_INTEGER,
                "last_run": {
                    "exit_code": MIN_EXIT_CODE,
                    "finished_at": MAX_SAFE_INTEGER,
                    "run_id": message_id(0xa1),
                    "script": "s".repeat(MAX_SCRIPT_BYTES),
                    "started_at": MAX_SAFE_INTEGER,
                    "state": "dead_letter",
                    "trigger": "scheduled"
                },
                "profile_revision": MAX_SAFE_INTEGER,
                "runner": {
                    "queue_depth": MAX_QUEUE_DEPTH,
                    "scheduler": "running",
                    "state": "degraded",
                    "workers_busy": MAX_WORKERS,
                    "workers_configured": MAX_WORKERS
                },
                "sequence": MAX_SAFE_INTEGER,
                "uptime_seconds": MAX_UPTIME_SECONDS
            }
        }),
        Kind::Signal => json!({
            "health_version": HEALTH_VERSION,
            "message_id": message_id(0x33),
            "target": target,
            "signal": {
                "kind": "run-completed",
                "occurred_at": MAX_SAFE_INTEGER,
                "run": {
                    "exit_code": MIN_EXIT_CODE,
                    "finished_at": MAX_SAFE_INTEGER,
                    "run_id": message_id(0xa1),
                    "script": "s".repeat(MAX_SCRIPT_BYTES),
                    "state": "dead_letter"
                },
                "sequence": MAX_SAFE_INTEGER,
                "signal_id": message_id(0xb1),
                "subject": Value::Null
            }
        }),
        Kind::Ack => json!({
            "health_version": HEALTH_VERSION,
            "message_id": message_id(0x44),
            "target": performer_id(),
            "ack": {
                "accepted": true,
                "acked_message_id": message_id(0x33),
                "cursor": MAX_SAFE_INTEGER
            }
        }),
        Kind::Error => json!({
            "health_version": HEALTH_VERSION,
            "message_id": message_id(0x55),
            "target": performer_id(),
            "error": {
                "accepted": false,
                "acked_message_id": message_id(0x33),
                "code": HealthCode::UnsupportedVersion.code(),
                "reason": HealthCode::UnsupportedVersion.name()
            }
        }),
    }
}

// ---------------------------------------------------------------------------
// Reference receiver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Peer {
    identity_key: [u8; 32],
    identity_active: bool,
    trust_active: bool,
    role: u8,
    capabilities: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PeerHealth {
    cursor: u64,
    last_pulse_sequence: u64,
    last_pulse_at: u64,
    last_profile_revision: u64,
    seen_signal_ids: HashSet<String>,
    stored_signals: usize,
    minute_messages: u32,
    minute_signals: u32,
    hour_profiles: u32,
}

struct Receiver {
    node_id: String,
    session_id_hex: String,
    peers: BTreeMap<String, Peer>,
    health: HashMap<String, PeerHealth>,
    seen_message_ids: HashSet<String>,
    global_signals: usize,
    now: u64,
}

impl Receiver {
    fn conductor() -> Self {
        let mut peers = BTreeMap::new();
        peers.insert(
            performer_id(),
            Peer {
                identity_key: x_only_public_key(PERFORMER_SCALAR_HEX),
                identity_active: true,
                trust_active: true,
                role: ROLE_PERFORMER,
                capabilities: vec![
                    CAPABILITY_PROFILE_PULSE.to_string(),
                    CAPABILITY_SIGNAL.to_string(),
                ],
            },
        );
        Self {
            node_id: conductor_id(),
            session_id_hex: SESSION_ID_HEX.to_string(),
            peers,
            health: HashMap::new(),
            seen_message_ids: HashSet::new(),
            global_signals: 0,
            now: BASE_NOW,
        }
    }

    fn performer() -> Self {
        let mut peers = BTreeMap::new();
        peers.insert(
            conductor_id(),
            Peer {
                identity_key: x_only_public_key(CONDUCTOR_SCALAR_HEX),
                identity_active: true,
                trust_active: true,
                role: ROLE_CONDUCTOR,
                capabilities: Vec::new(),
            },
        );
        Self {
            node_id: performer_id(),
            session_id_hex: SESSION_ID_HEX.to_string(),
            peers,
            health: HashMap::new(),
            seen_message_ids: HashSet::new(),
            global_signals: 0,
            now: BASE_NOW,
        }
    }

    fn peer_mut(&mut self, node: &str) -> &mut Peer {
        self.peers.get_mut(node).expect("configured peer")
    }

    /// The complete frozen receive order. Every branch returns a stable code and
    /// mutates nothing outside Health Plane state.
    fn accept(&mut self, encoded: &[u8]) -> Result<u64, HealthCode> {
        // Step 2: size, before parsing.
        if encoded.len() < SIGNATURE_BYTES {
            return Err(HealthCode::InvalidMessage);
        }
        let canonical_bytes = &encoded[..encoded.len() - SIGNATURE_BYTES];

        // Step 3: envelope shape and canonical re-encoding equality.
        let value: Value =
            serde_json::from_slice(canonical_bytes).map_err(|_| HealthCode::InvalidMessage)?;
        if canonical(&value) != canonical_bytes {
            return Err(HealthCode::InvalidMessage);
        }
        let envelope = value.as_object().ok_or(HealthCode::InvalidMessage)?;
        let envelope_fields = [
            "created_at",
            "kind",
            "nonce",
            "payload",
            "sender",
            "session_id",
            "version",
        ];
        exact_fields(envelope, &envelope_fields)?;
        if envelope["version"].as_u64() != Some(1) {
            return Err(HealthCode::InvalidMessage);
        }
        let kind_text = envelope["kind"]
            .as_str()
            .ok_or(HealthCode::InvalidMessage)?;
        let kind = Kind::parse(kind_text).ok_or(HealthCode::UnknownField)?;
        if canonical_bytes.len() > kind.max_canonical_bytes() {
            return Err(HealthCode::MessageTooLarge);
        }
        let sender = envelope["sender"]
            .as_str()
            .ok_or(HealthCode::InvalidMessage)?
            .to_string();
        let peer = self.peers.get(&sender).cloned();

        // Step 1 completion: production signature and session binding.
        let nonce = envelope_nonce(encoded).map_err(HealthCode::from_transport)?;
        let session_id: [u8; 32] = unhex(&self.session_id_hex)
            .try_into()
            .expect("session id length");
        let identity_key = peer
            .as_ref()
            .map(|peer| peer.identity_key)
            .ok_or(HealthCode::Revoked)?;
        verify_envelope(
            encoded,
            &sender,
            &identity_key,
            kind_text,
            &session_id,
            &nonce,
        )
        .map_err(HealthCode::from_transport)?;

        // Step 4: Health Plane version.
        let payload = envelope["payload"]
            .as_object()
            .ok_or(HealthCode::InvalidMessage)?;
        match payload.get("health_version").and_then(Value::as_u64) {
            None => return Err(HealthCode::UnknownField),
            Some(version) if version != HEALTH_VERSION => {
                return Err(HealthCode::UnsupportedVersion)
            }
            Some(_) => {}
        }

        // Step 5: strict closed schema.
        structural_bounds(&value)?;
        exact_fields(
            payload,
            &["health_version", "message_id", "target", kind.body_field()],
        )?;
        let message_id_text = hex16(payload.get("message_id"))?;
        let target = node_id_field(payload.get("target"))?;
        validate_body(kind, &payload[kind.body_field()], envelope)?;

        // Step 6: target binding.
        if target != self.node_id {
            return Err(HealthCode::WrongTarget);
        }

        // Step 7: trust.
        let peer = peer.ok_or(HealthCode::Revoked)?;
        if !peer.identity_active || !peer.trust_active {
            return Err(HealthCode::Revoked);
        }

        // Step 8: role.
        if peer.role != kind.required_role() {
            return Err(HealthCode::WrongRole);
        }

        // Step 9: capability.
        if let Some(required) = kind.required_capability() {
            if !peer.capabilities.iter().any(|value| value == required) {
                return Err(HealthCode::MissingCapability);
            }
        }

        // Step 10: freshness.
        let created_at = envelope["created_at"]
            .as_u64()
            .ok_or(HealthCode::InvalidMessage)?;
        if created_at > self.now.saturating_add(MAX_FUTURE_SKEW_SECONDS) {
            return Err(HealthCode::Future);
        }
        if self.now.saturating_sub(created_at) > MAX_AGE_SECONDS {
            return Err(HealthCode::Stale);
        }

        let now = self.now;
        let state = self.health.entry(sender.clone()).or_default();

        // Step 11: rate.
        if state.minute_messages >= MAX_MESSAGES_PER_PEER_PER_MINUTE + RATE_BURST_ALLOWANCE {
            return Err(HealthCode::RateLimited);
        }
        match kind {
            Kind::Profile if state.hour_profiles >= MAX_PROFILES_PER_PEER_PER_HOUR => {
                return Err(HealthCode::RateLimited)
            }
            Kind::Signal if state.minute_signals >= MAX_SIGNALS_PER_PEER_PER_MINUTE => {
                return Err(HealthCode::RateLimited)
            }
            Kind::Pulse
                if state.last_pulse_at > 0
                    && now.saturating_sub(state.last_pulse_at) < MIN_PULSE_INTERVAL_SECONDS =>
            {
                return Err(HealthCode::RateLimited)
            }
            _ => {}
        }

        // Step 12: replay.
        if self.seen_message_ids.contains(&message_id_text) {
            return Err(HealthCode::Replay);
        }

        // Step 13: ordering.
        let body = &payload[kind.body_field()];
        match kind {
            Kind::Profile => {
                let revision = body["profile_revision"].as_u64().unwrap_or_default();
                if revision <= state.last_profile_revision {
                    return Err(HealthCode::Replay);
                }
            }
            Kind::Pulse => {
                let sequence = body["sequence"].as_u64().unwrap_or_default();
                if sequence <= state.last_pulse_sequence {
                    return Err(HealthCode::Replay);
                }
            }
            Kind::Signal => {
                let sequence = body["sequence"].as_u64().unwrap_or_default();
                let signal_id = body["signal_id"].as_str().unwrap_or_default().to_string();
                if sequence <= state.cursor || state.seen_signal_ids.contains(&signal_id) {
                    return Err(HealthCode::Replay);
                }
                if sequence > state.cursor.saturating_add(REORDER_BUFFER_ENTRIES) {
                    return Err(HealthCode::Reordered);
                }
                if sequence != state.cursor + 1 {
                    // The contract holds this Signal in a 32-entry, 60-second
                    // reorder buffer. The reference receiver models that buffer
                    // with a zero lifetime, so the same stable code is produced
                    // immediately and the cursor still refuses to skip the gap.
                    return Err(HealthCode::Reordered);
                }
            }
            Kind::Ack | Kind::Error => {}
        }

        // Step 14: capacity.
        if kind == Kind::Signal
            && (state.stored_signals >= SIGNAL_INBOX_CAPACITY
                || self.global_signals >= SIGNAL_GLOBAL_INBOX_CAPACITY)
        {
            return Err(HealthCode::QueueFull);
        }

        // Step 15: apply.
        state.minute_messages += 1;
        match kind {
            Kind::Profile => {
                state.hour_profiles += 1;
                state.last_profile_revision = body["profile_revision"].as_u64().unwrap_or_default();
            }
            Kind::Pulse => {
                state.last_pulse_sequence = body["sequence"].as_u64().unwrap_or_default();
                state.last_pulse_at = now;
            }
            Kind::Signal => {
                state.minute_signals += 1;
                state.cursor = body["sequence"].as_u64().unwrap_or_default();
                state
                    .seen_signal_ids
                    .insert(body["signal_id"].as_str().unwrap_or_default().to_string());
                state.stored_signals += 1;
                self.global_signals += 1;
            }
            Kind::Ack | Kind::Error => {}
        }
        let cursor = self.health[&sender].cursor;
        self.seen_message_ids.insert(message_id_text);
        Ok(cursor)
    }
}

fn presence(last_pulse_age: u64, ever: bool) -> &'static str {
    if !ever {
        "unknown"
    } else if last_pulse_age <= PRESENCE_ONLINE_SECONDS {
        "online"
    } else if last_pulse_age <= PRESENCE_STALE_SECONDS {
        "stale"
    } else {
        "offline"
    }
}

// ---------------------------------------------------------------------------
// Strict schema validation
// ---------------------------------------------------------------------------

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> Result<(), HealthCode> {
    if object.len() != expected.len() {
        return Err(HealthCode::UnknownField);
    }
    for name in expected {
        if !object.contains_key(*name) {
            return Err(HealthCode::UnknownField);
        }
    }
    Ok(())
}

fn structural_bounds(value: &Value) -> Result<(), HealthCode> {
    fn walk(value: &Value, depth: usize, fields: &mut usize) -> Result<(), HealthCode> {
        if depth > MAX_JSON_DEPTH {
            return Err(HealthCode::InvalidMessage);
        }
        match value {
            Value::Object(map) => {
                for (name, child) in map {
                    if name.len() > MAX_FIELD_NAME_BYTES {
                        return Err(HealthCode::UnknownField);
                    }
                    *fields += 1;
                    if *fields > MAX_PAYLOAD_FIELDS {
                        return Err(HealthCode::InvalidMessage);
                    }
                    walk(child, depth + 1, fields)?;
                }
                Ok(())
            }
            Value::Array(items) => {
                if items.len() > MAX_ARRAY_LENGTH {
                    return Err(HealthCode::InvalidMessage);
                }
                for item in items {
                    walk(item, depth + 1, fields)?;
                }
                Ok(())
            }
            Value::String(text) => {
                if text.len() > MAX_STRING_BYTES
                    || text.starts_with("secret://")
                    || text.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
                {
                    return Err(HealthCode::InvalidMessage);
                }
                Ok(())
            }
            Value::Number(number) => {
                if number.is_f64() {
                    return Err(HealthCode::InvalidMessage);
                }
                Ok(())
            }
            Value::Bool(_) | Value::Null => Ok(()),
        }
    }
    let mut fields = 0;
    walk(value, 0, &mut fields)
}

fn grammar(text: &str, max: usize, allow_empty: bool, extra: &str) -> bool {
    if text.is_empty() {
        return allow_empty;
    }
    if text.len() > max {
        return false;
    }
    let first = text.as_bytes()[0];
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    text.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || extra.as_bytes().contains(&byte))
}

fn hex16(value: Option<&Value>) -> Result<String, HealthCode> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if text.len() != HEX16_CHARS
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(text.to_string())
}

fn node_id_field(value: Option<&Value>) -> Result<String, HealthCode> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if text.len() != NODE_ID_BYTES
        || !text.starts_with("omk1_")
        || !text[5..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(text.to_string())
}

fn bounded_u64(value: Option<&Value>, min: u64, max: u64) -> Result<u64, HealthCode> {
    let number = value
        .and_then(Value::as_u64)
        .ok_or(HealthCode::InvalidMessage)?;
    if number < min || number > max {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(number)
}

fn one_of(value: Option<&Value>, allowed: &[&str]) -> Result<String, HealthCode> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !allowed.contains(&text) {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(text.to_string())
}

fn exit_code(value: Option<&Value>) -> Result<(), HealthCode> {
    match value {
        Some(Value::Null) => Ok(()),
        Some(Value::Number(number)) => {
            let code = number.as_i64().ok_or(HealthCode::InvalidMessage)?;
            if !(MIN_EXIT_CODE..=MAX_EXIT_CODE).contains(&code) {
                return Err(HealthCode::InvalidMessage);
            }
            Ok(())
        }
        _ => Err(HealthCode::InvalidMessage),
    }
}

fn run_object(value: &Value, with_trigger: bool) -> Result<(), HealthCode> {
    let object = value.as_object().ok_or(HealthCode::InvalidMessage)?;
    let fields: &[&str] = if with_trigger {
        &[
            "exit_code",
            "finished_at",
            "run_id",
            "script",
            "started_at",
            "state",
            "trigger",
        ]
    } else {
        &["exit_code", "finished_at", "run_id", "script", "state"]
    };
    exact_fields(object, fields)?;
    exit_code(object.get("exit_code"))?;
    let finished_at = bounded_u64(object.get("finished_at"), 1, MAX_SAFE_INTEGER)?;
    hex16(object.get("run_id"))?;
    let script = object
        .get("script")
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !grammar(script, MAX_SCRIPT_BYTES, false, "._-") {
        return Err(HealthCode::InvalidMessage);
    }
    one_of(
        object.get("state"),
        &[
            "completed",
            "failed",
            "cancelled",
            "timed_out",
            "dead_letter",
        ],
    )?;
    if with_trigger {
        let started_at = bounded_u64(object.get("started_at"), 1, MAX_SAFE_INTEGER)?;
        if finished_at < started_at {
            return Err(HealthCode::InvalidMessage);
        }
        one_of(object.get("trigger"), &["manual", "scheduled", "queue"])?;
    }
    Ok(())
}

fn validate_body(
    kind: Kind,
    body: &Value,
    envelope: &Map<String, Value>,
) -> Result<(), HealthCode> {
    let object = body.as_object().ok_or(HealthCode::InvalidMessage)?;
    match kind {
        Kind::Profile => {
            exact_fields(
                object,
                &[
                    "agent_version",
                    "arch",
                    "baseline_id",
                    "baseline_observed_id",
                    "capabilities",
                    "display_name",
                    "distro_id",
                    "distro_version",
                    "omarchy_channel",
                    "omarchy_version",
                    "platform",
                    "profile_revision",
                    "role",
                    "runtimes",
                ],
            )?;
            let agent_version = object
                .get("agent_version")
                .and_then(Value::as_str)
                .ok_or(HealthCode::InvalidMessage)?;
            if !grammar(agent_version, MAX_AGENT_VERSION_BYTES, false, ".+-") {
                return Err(HealthCode::InvalidMessage);
            }
            one_of(object.get("arch"), &["x86_64", "aarch64", "unknown"])?;
            let mut baselines = Vec::with_capacity(2);
            for name in ["baseline_id", "baseline_observed_id"] {
                let text = object
                    .get(name)
                    .and_then(Value::as_str)
                    .ok_or(HealthCode::InvalidMessage)?;
                if !text.is_empty()
                    && (text.len() != BASELINE_ID_HEX_CHARS
                        || !text
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
                {
                    return Err(HealthCode::InvalidMessage);
                }
                baselines.push(text);
            }
            if baselines[0].is_empty() && !baselines[1].is_empty() {
                return Err(HealthCode::InvalidMessage);
            }
            let capabilities = object
                .get("capabilities")
                .and_then(Value::as_array)
                .ok_or(HealthCode::InvalidMessage)?;
            if capabilities.len() > MAX_CAPABILITY_COUNT {
                return Err(HealthCode::InvalidMessage);
            }
            let mut previous = "";
            for entry in capabilities {
                let text = entry.as_str().ok_or(HealthCode::InvalidMessage)?;
                if text.len() > MAX_CAPABILITY_BYTES
                    || !CAPABILITY_ALLOWLIST.contains(&text)
                    || text <= previous
                {
                    return Err(HealthCode::InvalidMessage);
                }
                previous = text;
            }
            let display_name = object
                .get("display_name")
                .and_then(Value::as_str)
                .ok_or(HealthCode::InvalidMessage)?;
            if !grammar(display_name, MAX_DISPLAY_NAME_BYTES, true, " ._-")
                || display_name.ends_with(' ')
            {
                return Err(HealthCode::InvalidMessage);
            }
            let distro_id = object
                .get("distro_id")
                .and_then(Value::as_str)
                .ok_or(HealthCode::InvalidMessage)?;
            if !grammar(distro_id, MAX_DISTRO_ID_BYTES, true, "._-")
                || distro_id.bytes().any(|byte| byte.is_ascii_uppercase())
            {
                return Err(HealthCode::InvalidMessage);
            }
            for name in ["distro_version", "omarchy_version"] {
                let text = object
                    .get(name)
                    .and_then(Value::as_str)
                    .ok_or(HealthCode::InvalidMessage)?;
                if !grammar(text, MAX_DISTRO_VERSION_BYTES, true, "._+-") {
                    return Err(HealthCode::InvalidMessage);
                }
            }
            one_of(object.get("omarchy_channel"), &["", "stable", "dev"])?;
            one_of(object.get("platform"), &["linux", "macos", "windows"])?;
            bounded_u64(object.get("profile_revision"), 1, MAX_SAFE_INTEGER)?;
            one_of(object.get("role"), &["performer"])?;
            let runtimes = object
                .get("runtimes")
                .and_then(Value::as_array)
                .ok_or(HealthCode::InvalidMessage)?;
            if runtimes.len() > MAX_RUNTIME_COUNT {
                return Err(HealthCode::InvalidMessage);
            }
            let mut previous_runtime = "";
            for entry in runtimes {
                let runtime = entry.as_object().ok_or(HealthCode::InvalidMessage)?;
                exact_fields(runtime, &["available", "name", "version"])?;
                let available = runtime
                    .get("available")
                    .and_then(Value::as_bool)
                    .ok_or(HealthCode::InvalidMessage)?;
                let name = one_of(runtime.get("name"), &RUNTIME_NAMES)?;
                if name.as_str() <= previous_runtime {
                    return Err(HealthCode::InvalidMessage);
                }
                let version = runtime
                    .get("version")
                    .and_then(Value::as_str)
                    .ok_or(HealthCode::InvalidMessage)?;
                if !grammar(version, MAX_DISTRO_VERSION_BYTES, true, "._+-")
                    || (!available && !version.is_empty())
                {
                    return Err(HealthCode::InvalidMessage);
                }
                previous_runtime = match name.as_str() {
                    "bash" => "bash",
                    "powershell" => "powershell",
                    "python" => "python",
                    _ => "sh",
                };
            }
            Ok(())
        }
        Kind::Pulse => {
            exact_fields(
                object,
                &[
                    "emitted_at",
                    "last_run",
                    "profile_revision",
                    "runner",
                    "sequence",
                    "uptime_seconds",
                ],
            )?;
            let emitted_at = bounded_u64(object.get("emitted_at"), 1, MAX_SAFE_INTEGER)?;
            if envelope["created_at"].as_u64() != Some(emitted_at) {
                return Err(HealthCode::InvalidMessage);
            }
            match object.get("last_run") {
                Some(Value::Null) => {}
                Some(value) => run_object(value, true)?,
                None => return Err(HealthCode::UnknownField),
            }
            bounded_u64(object.get("profile_revision"), 0, MAX_SAFE_INTEGER)?;
            let runner = object
                .get("runner")
                .and_then(Value::as_object)
                .ok_or(HealthCode::InvalidMessage)?;
            exact_fields(
                runner,
                &[
                    "queue_depth",
                    "scheduler",
                    "state",
                    "workers_busy",
                    "workers_configured",
                ],
            )?;
            bounded_u64(runner.get("queue_depth"), 0, MAX_QUEUE_DEPTH)?;
            one_of(runner.get("scheduler"), &["running", "disabled"])?;
            one_of(
                runner.get("state"),
                &["idle", "busy", "paused", "degraded", "stopped"],
            )?;
            let busy = bounded_u64(runner.get("workers_busy"), 0, MAX_WORKERS)?;
            let configured = bounded_u64(runner.get("workers_configured"), 0, MAX_WORKERS)?;
            if busy > configured {
                return Err(HealthCode::InvalidMessage);
            }
            bounded_u64(object.get("sequence"), 1, MAX_SAFE_INTEGER)?;
            bounded_u64(object.get("uptime_seconds"), 0, MAX_UPTIME_SECONDS)?;
            Ok(())
        }
        Kind::Signal => {
            exact_fields(
                object,
                &[
                    "kind",
                    "occurred_at",
                    "run",
                    "sequence",
                    "signal_id",
                    "subject",
                ],
            )?;
            let signal_kind = one_of(
                object.get("kind"),
                &["enrolled", "revoked", "run-completed"],
            )?;
            let occurred_at = bounded_u64(object.get("occurred_at"), 1, MAX_SAFE_INTEGER)?;
            if envelope["created_at"].as_u64().unwrap_or_default() < occurred_at {
                return Err(HealthCode::InvalidMessage);
            }
            let has_run = !matches!(object.get("run"), Some(Value::Null));
            let has_subject = !matches!(object.get("subject"), Some(Value::Null));
            match signal_kind.as_str() {
                "run-completed" if has_run && !has_subject => {
                    run_object(&object["run"], false)?;
                    let finished_at = object["run"]["finished_at"].as_u64().unwrap_or_default();
                    if finished_at != occurred_at {
                        return Err(HealthCode::InvalidMessage);
                    }
                }
                "enrolled" | "revoked" if has_subject && !has_run => {
                    node_id_field(object.get("subject"))?;
                }
                _ => return Err(HealthCode::InvalidMessage),
            }
            bounded_u64(object.get("sequence"), 1, MAX_SAFE_INTEGER)?;
            hex16(object.get("signal_id"))?;
            Ok(())
        }
        Kind::Ack => {
            exact_fields(object, &["accepted", "acked_message_id", "cursor"])?;
            if object.get("accepted").and_then(Value::as_bool) != Some(true) {
                return Err(HealthCode::InvalidMessage);
            }
            hex16(object.get("acked_message_id"))?;
            bounded_u64(object.get("cursor"), 0, MAX_SAFE_INTEGER)?;
            Ok(())
        }
        Kind::Error => {
            exact_fields(object, &["accepted", "acked_message_id", "code", "reason"])?;
            if object.get("accepted").and_then(Value::as_bool) != Some(false) {
                return Err(HealthCode::InvalidMessage);
            }
            hex16(object.get("acked_message_id"))?;
            let code = bounded_u64(object.get("code"), 1101, 1115)? as u16;
            let expected = HealthCode::ALL
                .into_iter()
                .find(|candidate| candidate.code() == code)
                .ok_or(HealthCode::InvalidMessage)?;
            one_of(object.get("reason"), &[expected.name()])?;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture access
// ---------------------------------------------------------------------------

fn fixture() -> toml::Value {
    toml::from_str(include_str!("fixtures/health_plane_vectors.toml"))
        .expect("health plane fixture must parse")
}

fn integer(fixture: &toml::Value, key: &str) -> i64 {
    fixture[key]
        .as_integer()
        .unwrap_or_else(|| panic!("fixture key {key} must be an integer"))
}

fn text<'a>(fixture: &'a toml::Value, key: &str) -> &'a str {
    fixture[key]
        .as_str()
        .unwrap_or_else(|| panic!("fixture key {key} must be a string"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fixture_pins_every_frozen_bound() {
    let fixture = fixture();

    assert_eq!(integer(&fixture, "format_version"), 1);
    assert_eq!(text(&fixture, "contract_id"), CONTRACT_ID);
    assert_eq!(text(&fixture, "status"), "frozen-pending-owner-review");
    assert_eq!(
        text(&fixture, "contract_document"),
        "docs/internal/health-plane-contract.md"
    );
    assert_eq!(integer(&fixture, "health_version"), HEALTH_VERSION as i64);
    assert_eq!(integer(&fixture, "envelope_version"), 1);
    assert_eq!(
        text(&fixture, "envelope_signature_domain_hex"),
        hex(ENVELOPE_DOMAIN)
    );
    assert_eq!(
        integer(&fixture, "registry_schema_version"),
        REGISTRY_SCHEMA_VERSION
    );
    // The registry schema is at least as new as the frozen Health Plane
    // version, so the shipped implementation cannot lag the contract. The
    // frozen value itself, 7, is asserted against the fixture immediately
    // above and is unchanged.
    const {
        assert!(
            REGISTRY_SCHEMA_VERSION >= omakure::node_registry::SCHEMA_VERSION,
            "the registry schema must never lag the frozen Health Plane version"
        )
    };

    let pairs: [(&str, i64); 52] = [
        ("role_conductor", ROLE_CONDUCTOR as i64),
        ("role_performer", ROLE_PERFORMER as i64),
        ("max_canonical_profile_bytes", MAX_CANONICAL_PROFILE as i64),
        ("max_canonical_pulse_bytes", MAX_CANONICAL_PULSE as i64),
        ("max_canonical_signal_bytes", MAX_CANONICAL_SIGNAL as i64),
        ("max_canonical_ack_bytes", MAX_CANONICAL_ACK as i64),
        ("max_canonical_error_bytes", MAX_CANONICAL_ERROR as i64),
        ("signature_bytes", SIGNATURE_BYTES as i64),
        ("max_json_depth", MAX_JSON_DEPTH as i64),
        ("max_field_name_bytes", MAX_FIELD_NAME_BYTES as i64),
        ("max_payload_fields", MAX_PAYLOAD_FIELDS as i64),
        ("max_array_length", MAX_ARRAY_LENGTH as i64),
        ("max_string_bytes", MAX_STRING_BYTES as i64),
        ("max_safe_integer", MAX_SAFE_INTEGER as i64),
        ("node_id_bytes", NODE_ID_BYTES as i64),
        ("opaque_id_hex_chars", HEX16_CHARS as i64),
        ("max_agent_version_bytes", MAX_AGENT_VERSION_BYTES as i64),
        ("max_display_name_bytes", MAX_DISPLAY_NAME_BYTES as i64),
        ("max_distro_id_bytes", MAX_DISTRO_ID_BYTES as i64),
        ("max_distro_version_bytes", MAX_DISTRO_VERSION_BYTES as i64),
        ("max_script_bytes", MAX_SCRIPT_BYTES as i64),
        ("max_capability_bytes", MAX_CAPABILITY_BYTES as i64),
        ("max_capability_count", MAX_CAPABILITY_COUNT as i64),
        ("max_runtime_count", MAX_RUNTIME_COUNT as i64),
        ("max_workers", MAX_WORKERS as i64),
        ("max_queue_depth", MAX_QUEUE_DEPTH as i64),
        ("max_uptime_seconds", MAX_UPTIME_SECONDS as i64),
        ("min_exit_code", MIN_EXIT_CODE),
        ("max_exit_code", MAX_EXIT_CODE),
        ("max_age_seconds", MAX_AGE_SECONDS as i64),
        ("max_future_skew_seconds", MAX_FUTURE_SKEW_SECONDS as i64),
        ("presence_online_seconds", PRESENCE_ONLINE_SECONDS as i64),
        ("presence_stale_seconds", PRESENCE_STALE_SECONDS as i64),
        (
            "nominal_pulse_interval_seconds",
            NOMINAL_PULSE_INTERVAL_SECONDS as i64,
        ),
        (
            "min_pulse_interval_seconds",
            MIN_PULSE_INTERVAL_SECONDS as i64,
        ),
        (
            "max_messages_per_peer_per_minute",
            MAX_MESSAGES_PER_PEER_PER_MINUTE as i64,
        ),
        (
            "max_profiles_per_peer_per_hour",
            MAX_PROFILES_PER_PEER_PER_HOUR as i64,
        ),
        (
            "max_signals_per_peer_per_minute",
            MAX_SIGNALS_PER_PEER_PER_MINUTE as i64,
        ),
        ("rate_burst_allowance", RATE_BURST_ALLOWANCE as i64),
        (
            "max_in_flight_per_session",
            MAX_IN_FLIGHT_PER_SESSION as i64,
        ),
        (
            "max_performers_per_conductor",
            MAX_PERFORMERS_PER_CONDUCTOR as i64,
        ),
        (
            "max_conductors_per_performer",
            MAX_CONDUCTORS_PER_PERFORMER as i64,
        ),
        ("signal_outbox_capacity", SIGNAL_OUTBOX_CAPACITY as i64),
        ("signal_inbox_capacity", SIGNAL_INBOX_CAPACITY as i64),
        (
            "signal_global_inbox_capacity",
            SIGNAL_GLOBAL_INBOX_CAPACITY as i64,
        ),
        ("ack_timeout_seconds", ACK_TIMEOUT_SECONDS as i64),
        ("max_retries", MAX_RETRIES as i64),
        ("processing_budget_millis", PROCESSING_BUDGET_MILLIS as i64),
        ("reorder_buffer_entries", REORDER_BUFFER_ENTRIES as i64),
        ("reorder_buffer_seconds", REORDER_BUFFER_SECONDS as i64),
        (
            "replay_security_floor_seconds",
            REPLAY_SECURITY_FLOOR_SECONDS as i64,
        ),
        ("replay_retention_seconds", REPLAY_RETENTION_SECONDS as i64),
    ];
    for (key, expected) in pairs {
        assert_eq!(integer(&fixture, key), expected, "fixture bound {key}");
    }

    let storage: [(&str, i64); 11] = [
        ("max_replay_rows", MAX_REPLAY_ROWS as i64),
        ("replay_row_bytes", REPLAY_ROW_BYTES as i64),
        ("signal_retention_seconds", SIGNAL_RETENTION_SECONDS as i64),
        ("max_stored_profile_bytes", MAX_STORED_PROFILE_BYTES as i64),
        ("max_stored_pulse_bytes", MAX_STORED_PULSE_BYTES as i64),
        ("max_stored_signal_bytes", MAX_STORED_SIGNAL_BYTES as i64),
        ("max_audit_rows", MAX_AUDIT_ROWS as i64),
        ("audit_row_bytes", AUDIT_ROW_BYTES as i64),
        ("audit_retention_seconds", AUDIT_RETENTION_SECONDS as i64),
        (
            "version_incompatible_backoff_seconds",
            VERSION_INCOMPATIBLE_BACKOFF_SECONDS as i64,
        ),
        (
            "version_incompatible_expiry_seconds",
            VERSION_INCOMPATIBLE_EXPIRY_SECONDS as i64,
        ),
    ];
    for (key, expected) in storage {
        assert_eq!(integer(&fixture, key), expected, "fixture bound {key}");
    }

    let backoff: Vec<i64> = fixture["retry_backoff_seconds"]
        .as_array()
        .expect("retry backoff array")
        .iter()
        .map(|value| value.as_integer().expect("backoff integer"))
        .collect();
    assert_eq!(
        backoff,
        RETRY_BACKOFF_SECONDS
            .iter()
            .map(|value| *value as i64)
            .collect::<Vec<_>>()
    );

    let allowlist: Vec<&str> = fixture["capability_allowlist"]
        .as_array()
        .expect("capability allowlist")
        .iter()
        .map(|value| value.as_str().expect("capability string"))
        .collect();
    assert_eq!(allowlist, CAPABILITY_ALLOWLIST);
    assert_eq!(
        text(&fixture, "capability_profile_pulse"),
        CAPABILITY_PROFILE_PULSE
    );
    assert_eq!(text(&fixture, "capability_signal"), CAPABILITY_SIGNAL);

    let runtimes: Vec<&str> = fixture["runtime_names"]
        .as_array()
        .expect("runtime names")
        .iter()
        .map(|value| value.as_str().expect("runtime name string"))
        .collect();
    assert_eq!(runtimes, RUNTIME_NAMES);
    // The Performer clamps to this list and the receiver validates against it.
    // Were the shipped constant to drift from the frozen contract, a Profile
    // would be built from one allow-list and rejected against another.
    assert_eq!(
        omakure::health_plane::bounds::RUNTIME_NAMES,
        RUNTIME_NAMES,
        "the shipped runtime allow-list must equal the frozen contract"
    );
    assert!(
        fixture["new_capability_required"]
            .as_bool()
            .expect("new capability claim"),
        "the contract must record that a new capability is required"
    );
}

#[test]
fn storage_ceiling_is_the_sum_of_its_frozen_parts() {
    let fixture = fixture();
    let per_performer = MAX_STORED_PROFILE_BYTES
        + MAX_STORED_PULSE_BYTES
        + (SIGNAL_INBOX_CAPACITY as u64) * MAX_STORED_SIGNAL_BYTES;
    assert_eq!(per_performer, 73_088);
    let payload = per_performer * MAX_PERFORMERS_PER_CONDUCTOR as u64;
    assert_eq!(payload, 18_710_528);
    let replay = MAX_REPLAY_ROWS * REPLAY_ROW_BYTES;
    assert_eq!(replay, 4_194_304);
    let audit = MAX_AUDIT_ROWS * AUDIT_ROW_BYTES;
    assert_eq!(audit, 2_560_000);
    assert_eq!(payload + replay + audit, STORAGE_CEILING_BYTES);
    assert_eq!(
        integer(&fixture, "storage_ceiling_bytes"),
        STORAGE_CEILING_BYTES as i64
    );
    assert_eq!(
        integer(&fixture, "worst_case_bytes_per_performer"),
        per_performer as i64
    );

    // The Health Plane must never be able to exhaust the frozen transport.
    const { assert!(MAX_PERFORMERS_PER_CONDUCTOR < 1_024) };
    // Replay rows must outlive the freshness window by a wide margin.
    const { assert!(REPLAY_RETENTION_SECONDS > REPLAY_SECURITY_FLOOR_SECONDS) };
    assert_eq!(
        REPLAY_SECURITY_FLOOR_SECONDS,
        MAX_AGE_SECONDS + MAX_FUTURE_SKEW_SECONDS
    );
    // Presence thresholds must bracket the nominal Pulse interval.
    assert_eq!(PRESENCE_ONLINE_SECONDS, NOMINAL_PULSE_INTERVAL_SECONDS * 3);
    const { assert!(PRESENCE_STALE_SECONDS > PRESENCE_ONLINE_SECONDS) };
}

#[test]
fn error_codes_are_stable_disjoint_and_auditable() {
    let fixture = fixture();
    let rows = fixture["error_codes"]
        .as_array()
        .expect("error code table")
        .iter()
        .map(|row| {
            (
                row["code"].as_integer().expect("code") as u16,
                row["name"].as_str().expect("name").to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), HealthCode::ALL.len());
    for (index, code) in HealthCode::ALL.into_iter().enumerate() {
        assert_eq!(rows[index].0, code.code());
        assert_eq!(rows[index].1, code.name());
        // Disjoint from the frozen transport codes and inside the audit range.
        assert!((1101..=1115).contains(&code.code()));
        assert!(!(1001..=1011).contains(&code.code()));
        assert!((1000..=1999).contains(&code.code()));
    }
}

#[test]
fn reference_vectors_are_canonical_and_verify_through_the_production_path() {
    let fixture = fixture();
    let vectors = fixture["vectors"].as_array().expect("vector table");
    assert_eq!(vectors.len(), Kind::ALL.len());

    for (index, kind) in Kind::ALL.into_iter().enumerate() {
        let vector = &vectors[index];
        assert_eq!(vector["kind"].as_str(), Some(kind.wire()));

        let encoded = reference_message(kind);
        let canonical_bytes = &encoded[..encoded.len() - SIGNATURE_BYTES];
        let signature = &encoded[encoded.len() - SIGNATURE_BYTES..];

        assert_eq!(
            hex(canonical_bytes),
            vector["canonical_hex"].as_str().expect("canonical hex"),
            "{} canonical bytes drifted",
            kind.wire()
        );
        assert_eq!(
            hex(signature),
            vector["signature_hex"].as_str().expect("signature hex"),
            "{} signature drifted",
            kind.wire()
        );
        assert_eq!(
            canonical_bytes.len() as i64,
            vector["canonical_bytes"].as_integer().expect("length"),
        );

        // Canonical re-encoding is idempotent (RFC-8785).
        let parsed: Value = serde_json::from_slice(canonical_bytes).expect("parse canonical");
        assert_eq!(canonical(&parsed), canonical_bytes);

        // The production verifier accepts it without any Health Plane code.
        let nonce = envelope_nonce(&encoded).expect("nonce");
        let session_id: [u8; 32] = unhex(SESSION_ID_HEX).try_into().unwrap();
        verify_envelope(
            &encoded,
            &node_id(reference_scalar(kind)),
            &x_only_public_key(reference_scalar(kind)),
            kind.wire(),
            &session_id,
            &nonce,
        )
        .expect("production verify_envelope must accept a canonical Health Plane envelope");
    }
}

#[test]
fn worst_case_messages_stay_inside_their_frozen_caps() {
    let fixture = fixture();
    let measured = fixture["worst_case_canonical_bytes"]
        .as_table()
        .expect("worst case table");
    for kind in Kind::ALL {
        let (canonical_bytes, _) = sign_envelope(
            reference_scalar(kind),
            kind.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            worst_case_payload(kind),
            MAX_SAFE_INTEGER,
        );
        assert!(
            canonical_bytes.len() <= kind.max_canonical_bytes(),
            "{} worst case {} exceeds cap {}",
            kind.wire(),
            canonical_bytes.len(),
            kind.max_canonical_bytes()
        );
        assert_eq!(
            measured[kind.wire()].as_integer().expect("measured"),
            canonical_bytes.len() as i64,
            "{} worst case drifted",
            kind.wire()
        );
        // The encoded message must stay far under the frozen plaintext limit.
        assert!(canonical_bytes.len() + SIGNATURE_BYTES < 1_048_520);
    }
}

#[test]
fn canonical_messages_are_accepted_for_every_kind() {
    for kind in [Kind::Profile, Kind::Pulse, Kind::Signal] {
        let mut receiver = Receiver::conductor();
        assert_eq!(
            receiver.accept(&reference_message(kind)),
            Ok(if kind == Kind::Signal { 1 } else { 0 }),
            "{} must be accepted",
            kind.wire()
        );
    }
    for kind in [Kind::Ack, Kind::Error] {
        let mut receiver = Receiver::performer();
        assert_eq!(
            receiver.accept(&reference_message(kind)),
            Ok(0),
            "{} must be accepted",
            kind.wire()
        );
    }
}

#[test]
fn full_reporting_sequence_advances_state_exactly_once() {
    let mut receiver = Receiver::conductor();
    let conductor = conductor_id();

    let (canonical_bytes, signature) = sign_envelope(
        PERFORMER_SCALAR_HEX,
        Kind::Profile.wire(),
        SESSION_ID_HEX,
        &nonce_hex(0x01),
        profile_payload(&conductor, 1),
        BASE_NOW,
    );
    assert_eq!(
        receiver.accept(&encode(&canonical_bytes, &signature)),
        Ok(0)
    );

    for sequence in 1..=3_u64 {
        receiver.now = BASE_NOW + sequence * NOMINAL_PULSE_INTERVAL_SECONDS;
        let created = receiver.now;
        let mut payload = pulse_payload(&conductor, sequence, created);
        payload["message_id"] = Value::from(hex(&[0x60 + sequence as u8; 16]));
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            created,
        );
        assert_eq!(
            receiver.accept(&encode(&canonical_bytes, &signature)),
            Ok(0),
            "pulse {sequence} must be accepted"
        );
    }

    for sequence in 1..=4_u64 {
        let mut payload = signal_payload(&conductor, sequence, 0xb0 + sequence as u8);
        payload["message_id"] = Value::from(hex(&[0x70 + sequence as u8; 16]));
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Signal.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            receiver.now,
        );
        assert_eq!(
            receiver.accept(&encode(&canonical_bytes, &signature)),
            Ok(sequence),
            "signal {sequence} must advance the cursor by exactly one"
        );
    }

    let state = &receiver.health[&performer_id()];
    assert_eq!(state.cursor, 4);
    assert_eq!(state.stored_signals, 4);
    assert_eq!(state.last_profile_revision, 1);
    assert_eq!(state.last_pulse_sequence, 3);
    assert_eq!(receiver.global_signals, 4);
}

#[test]
fn every_contracted_adversarial_case_is_rejected_with_its_stable_code() {
    let conductor = conductor_id();
    let performer = performer_id();

    let mut observed: Vec<(&'static str, HealthCode)> = Vec::new();
    let mut record = |name: &'static str, expected: HealthCode, actual: Result<u64, HealthCode>| {
        let actual = actual.unwrap_err();
        assert_eq!(actual, expected, "{name} produced the wrong stable code");
        observed.push((name, actual));
    };

    // 1101 unsupported version.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = pulse_payload(&conductor, 1, BASE_NOW);
        payload["health_version"] = Value::from(2);
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "unknown-version",
            HealthCode::UnsupportedVersion,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1102 malformed: non-canonical byte order.
    {
        let mut receiver = Receiver::conductor();
        let encoded = reference_message(Kind::Pulse);
        let canonical_bytes = &encoded[..encoded.len() - SIGNATURE_BYTES];
        let text = String::from_utf8(canonical_bytes.to_vec()).unwrap();
        let mangled = text.replacen("{\"created_at\"", "{ \"created_at\"", 1);
        let mut broken = mangled.into_bytes();
        broken.extend_from_slice(&encoded[encoded.len() - SIGNATURE_BYTES..]);
        record(
            "non-canonical-json",
            HealthCode::InvalidMessage,
            receiver.accept(&broken),
        );
    }

    // 1102 malformed: mutated signature.
    {
        let mut receiver = Receiver::conductor();
        let mut encoded = reference_message(Kind::Pulse);
        *encoded.last_mut().unwrap() ^= 1;
        record(
            "mutated-signature",
            HealthCode::InvalidMessage,
            receiver.accept(&encoded),
        );
    }

    // 1102 malformed: field combination.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = signal_payload(&conductor, 1, 0xb1);
        payload["signal"]["subject"] = Value::from(performer.clone());
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Signal.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "signal-run-and-subject",
            HealthCode::InvalidMessage,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1102 privacy: a secret reference anywhere.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = profile_payload(&conductor, 1);
        payload["profile"]["display_name"] = Value::from("secret://vault/api-token");
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Profile.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "secret-reference",
            HealthCode::InvalidMessage,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1102 privacy: a filesystem path in a bounded field.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = pulse_payload(&conductor, 1, BASE_NOW);
        payload["pulse"]["last_run"]["script"] = Value::from("/home/operator/scripts/deploy.sh");
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "workspace-path",
            HealthCode::InvalidMessage,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1103 oversized: a Profile-sized body carried under a Signal's smaller cap.
    {
        let mut receiver = Receiver::conductor();
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Signal.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            worst_case_payload(Kind::Profile),
            BASE_NOW,
        );
        assert!(canonical_bytes.len() > MAX_CANONICAL_SIGNAL);
        record(
            "oversized-for-kind",
            HealthCode::MessageTooLarge,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1104 wrong target.
    {
        let mut receiver = Receiver::conductor();
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            pulse_payload(&performer, 1, BASE_NOW),
            BASE_NOW,
        );
        record(
            "wrong-target",
            HealthCode::WrongTarget,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1105 wrong role: a conductor-role peer reporting health.
    {
        let mut receiver = Receiver::conductor();
        receiver.peer_mut(&performer).role = ROLE_CONDUCTOR;
        record(
            "wrong-role",
            HealthCode::WrongRole,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1105 wrong role: a performer acknowledging.
    {
        let mut receiver = Receiver::performer();
        receiver.peer_mut(&conductor).role = ROLE_PERFORMER;
        record(
            "wrong-direction-ack",
            HealthCode::WrongRole,
            receiver.accept(&reference_message(Kind::Ack)),
        );
    }

    // 1106 missing capability.
    {
        let mut receiver = Receiver::conductor();
        receiver.peer_mut(&performer).capabilities = vec![CAPABILITY_PROFILE_PULSE.to_string()];
        record(
            "missing-capability",
            HealthCode::MissingCapability,
            receiver.accept(&reference_message(Kind::Signal)),
        );
    }

    // 1107 revoked trust.
    {
        let mut receiver = Receiver::conductor();
        receiver.peer_mut(&performer).trust_active = false;
        record(
            "revoked-trust",
            HealthCode::Revoked,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1107 revoked identity.
    {
        let mut receiver = Receiver::conductor();
        receiver.peer_mut(&performer).identity_active = false;
        record(
            "revoked-identity",
            HealthCode::Revoked,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1107 unknown peer.
    {
        let mut receiver = Receiver::conductor();
        receiver.peers.clear();
        record(
            "unknown-peer",
            HealthCode::Revoked,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1108 stale: one second past the inclusive boundary.
    {
        let mut receiver = Receiver::conductor();
        receiver.now = BASE_NOW + MAX_AGE_SECONDS + 1;
        record(
            "stale",
            HealthCode::Stale,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1109 future: one second past the inclusive boundary.
    {
        let mut receiver = Receiver::conductor();
        receiver.now = BASE_NOW - MAX_FUTURE_SKEW_SECONDS - 1;
        record(
            "future",
            HealthCode::Future,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1110 replay: the same message twice. The clock is advanced past the
    // minimum Pulse interval so the rate check at step 11 cannot mask the
    // replay check at step 12.
    {
        let mut receiver = Receiver::conductor();
        assert_eq!(receiver.accept(&reference_message(Kind::Pulse)), Ok(0));
        receiver.now = BASE_NOW + MIN_PULSE_INTERVAL_SECONDS;
        record(
            "replayed-message-id",
            HealthCode::Replay,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1110 replay: a cross-session envelope.
    {
        let mut receiver = Receiver::conductor();
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            OTHER_SESSION_ID_HEX,
            &nonce_hex(0x01),
            pulse_payload(&conductor, 1, BASE_NOW),
            BASE_NOW,
        );
        record(
            "cross-session",
            HealthCode::Replay,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1110 replay: a duplicated signal_id under a fresh message_id.
    {
        let mut receiver = Receiver::conductor();
        assert_eq!(receiver.accept(&reference_message(Kind::Signal)), Ok(1));
        let mut payload = signal_payload(&conductor, 2, 0xb1);
        payload["message_id"] = Value::from(hex(&[0x99; 16]));
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Signal.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "replayed-signal-id",
            HealthCode::Replay,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1110 replay: a non-advancing pulse sequence under a fresh message id.
    {
        let mut receiver = Receiver::conductor();
        assert_eq!(receiver.accept(&reference_message(Kind::Pulse)), Ok(0));
        receiver.now = BASE_NOW + MIN_PULSE_INTERVAL_SECONDS;
        let mut payload = pulse_payload(&conductor, 1, BASE_NOW);
        payload["message_id"] = Value::from(hex(&[0x98; 16]));
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "stalled-pulse-sequence",
            HealthCode::Replay,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1111 reordered: a gap the cursor may not skip.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = signal_payload(&conductor, 2, 0xb2);
        payload["message_id"] = Value::from(hex(&[0x97; 16]));
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Signal.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "reordered-gap",
            HealthCode::Reordered,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1111 reordered: beyond the reorder window.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = signal_payload(&conductor, REORDER_BUFFER_ENTRIES + 2, 0xb3);
        payload["message_id"] = Value::from(hex(&[0x96; 16]));
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Signal.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "reordered-far-future",
            HealthCode::Reordered,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1112 rate limited.
    {
        let mut receiver = Receiver::conductor();
        receiver.health.insert(
            performer.clone(),
            PeerHealth {
                minute_messages: MAX_MESSAGES_PER_PEER_PER_MINUTE + RATE_BURST_ALLOWANCE,
                ..PeerHealth::default()
            },
        );
        record(
            "rate-limited",
            HealthCode::RateLimited,
            receiver.accept(&reference_message(Kind::Pulse)),
        );
    }

    // 1113 queue full.
    {
        let mut receiver = Receiver::conductor();
        receiver.health.insert(
            performer.clone(),
            PeerHealth {
                stored_signals: SIGNAL_INBOX_CAPACITY,
                ..PeerHealth::default()
            },
        );
        record(
            "inbox-full",
            HealthCode::QueueFull,
            receiver.accept(&reference_message(Kind::Signal)),
        );
    }

    // 1114 unknown field.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = pulse_payload(&conductor, 1, BASE_NOW);
        payload["pulse"]["cpu_percent"] = Value::from(42);
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "unknown-field",
            HealthCode::UnknownField,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1114 missing field.
    {
        let mut receiver = Receiver::conductor();
        let mut payload = pulse_payload(&conductor, 1, BASE_NOW);
        payload["pulse"]
            .as_object_mut()
            .unwrap()
            .remove("uptime_seconds");
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Pulse.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        record(
            "missing-field",
            HealthCode::UnknownField,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    // 1114 unknown health kind.
    {
        let mut receiver = Receiver::conductor();
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            "health_inventory",
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            pulse_payload(&conductor, 1, BASE_NOW),
            BASE_NOW,
        );
        record(
            "unknown-kind",
            HealthCode::UnknownField,
            receiver.accept(&encode(&canonical_bytes, &signature)),
        );
    }

    let covered: HashSet<u16> = observed.iter().map(|(_, code)| code.code()).collect();
    for code in HealthCode::ALL {
        if code == HealthCode::CorruptState {
            // 1115 is a local-state outcome, covered by the corruption rules
            // rather than by a wire vector.
            continue;
        }
        assert!(
            covered.contains(&code.code()),
            "no adversarial vector covers {}",
            code.name()
        );
    }
    assert!(observed.len() >= 25, "adversarial coverage shrank");
}

#[test]
fn boundaries_are_inclusive_exactly_where_the_contract_says() {
    // Freshness: the oldest and newest accepted instants.
    let mut receiver = Receiver::conductor();
    receiver.now = BASE_NOW + MAX_AGE_SECONDS;
    assert_eq!(receiver.accept(&reference_message(Kind::Pulse)), Ok(0));

    let mut receiver = Receiver::conductor();
    receiver.now = BASE_NOW - MAX_FUTURE_SKEW_SECONDS;
    assert_eq!(receiver.accept(&reference_message(Kind::Pulse)), Ok(0));

    // Signal reorder window: the last acceptable in-order sequence.
    let mut receiver = Receiver::conductor();
    assert_eq!(receiver.accept(&reference_message(Kind::Signal)), Ok(1));

    // Presence classification boundaries.
    assert_eq!(presence(0, false), "unknown");
    assert_eq!(presence(0, true), "online");
    assert_eq!(presence(PRESENCE_ONLINE_SECONDS, true), "online");
    assert_eq!(presence(PRESENCE_ONLINE_SECONDS + 1, true), "stale");
    assert_eq!(presence(PRESENCE_STALE_SECONDS, true), "stale");
    assert_eq!(presence(PRESENCE_STALE_SECONDS + 1, true), "offline");
}

#[test]
fn privacy_classes_are_closed_and_forbid_every_listed_disclosure() {
    let fixture = fixture();
    let permitted: Vec<&str> = fixture["privacy_p0_permitted"]
        .as_array()
        .expect("p0 list")
        .iter()
        .map(|value| value.as_str().expect("p0 entry"))
        .collect();
    let forbidden: Vec<&str> = fixture["privacy_p1_forbidden"]
        .as_array()
        .expect("p1 list")
        .iter()
        .map(|value| value.as_str().expect("p1 entry"))
        .collect();

    for required in [
        "secret_values",
        "bearer_tokens",
        "private_keys",
        "raw_script_arguments",
        "script_output",
        "workspace_paths",
        "filesystem_paths",
        "hostnames",
        "usernames",
        "ip_addresses",
        "host_inventory",
        "resource_gauges",
    ] {
        assert!(
            forbidden.contains(&required),
            "privacy class P1 must forbid {required}"
        );
    }
    for banned in forbidden.iter() {
        assert!(
            !permitted.contains(banned),
            "{banned} cannot be both permitted and forbidden"
        );
    }

    // Every field name the schema accepts must be a declared P0 fact.
    let declared: HashSet<&str> = permitted.iter().copied().collect();
    for name in [
        "agent_version",
        "arch",
        "baseline_id",
        "baseline_observed_id",
        "capabilities",
        "display_name",
        "distro_id",
        "distro_version",
        "omarchy_channel",
        "omarchy_version",
        "platform",
        "profile_revision",
        "role",
        "runtimes",
        "queue_depth",
        "scheduler",
        "workers_busy",
        "workers_configured",
        "uptime_seconds",
        "sequence",
        "cursor",
        "script",
        "run_id",
        "exit_code",
        "signal_id",
        "message_id",
        "target",
    ] {
        assert!(declared.contains(name), "{name} is not a declared P0 fact");
    }

    // No accepted string field may express a path, URL, or credential shape.
    for character in ['/', '\\', ':', '@'] {
        let mut receiver = Receiver::conductor();
        let mut payload = profile_payload(&conductor_id(), 1);
        payload["profile"]["distro_id"] = Value::from(format!("arch{character}etc"));
        let (canonical_bytes, signature) = sign_envelope(
            PERFORMER_SCALAR_HEX,
            Kind::Profile.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            payload,
            BASE_NOW,
        );
        assert_eq!(
            receiver.accept(&encode(&canonical_bytes, &signature)),
            Err(HealthCode::InvalidMessage),
            "character {character} must be rejected in a bounded field"
        );
    }
}

#[test]
fn authorization_never_reads_the_message_and_no_new_capability_is_needed() {
    // A Profile that claims every capability is still authorized only by the
    // receiver's own registry row.
    let mut receiver = Receiver::conductor();
    receiver.peer_mut(&performer_id()).capabilities = Vec::new();
    let mut payload = profile_payload(&conductor_id(), 1);
    payload["profile"]["capabilities"] = json!(CAPABILITY_ALLOWLIST);
    let (canonical_bytes, signature) = sign_envelope(
        PERFORMER_SCALAR_HEX,
        Kind::Profile.wire(),
        SESSION_ID_HEX,
        &nonce_hex(0x01),
        payload,
        BASE_NOW,
    );
    assert_eq!(
        receiver.accept(&encode(&canonical_bytes, &signature)),
        Err(HealthCode::MissingCapability),
        "a self-declared capability must never authorize"
    );

    // The two required capabilities are already in the frozen allow-list.
    assert!(CAPABILITY_ALLOWLIST.contains(&CAPABILITY_PROFILE_PULSE));
    assert!(CAPABILITY_ALLOWLIST.contains(&CAPABILITY_SIGNAL));
    for kind in Kind::ALL {
        if let Some(required) = kind.required_capability() {
            assert!(
                CAPABILITY_ALLOWLIST.contains(&required),
                "{} requires a capability outside the frozen allow-list",
                kind.wire()
            );
        }
    }

    // A performer that grants inventory-health but not notifications reports
    // Profile and Pulse and refuses Signals.
    let mut receiver = Receiver::conductor();
    receiver.peer_mut(&performer_id()).capabilities = vec![CAPABILITY_PROFILE_PULSE.to_string()];
    assert_eq!(receiver.accept(&reference_message(Kind::Profile)), Ok(0));
    assert_eq!(receiver.accept(&reference_message(Kind::Pulse)), Ok(0));
    assert_eq!(
        receiver.accept(&reference_message(Kind::Signal)),
        Err(HealthCode::MissingCapability)
    );
}

/// Regenerates the pinned hex in `tests/fixtures/health_plane_vectors.toml`.
/// Run with `cargo test --test health_plane_contract -- --ignored --nocapture`.
#[test]
#[ignore = "vector generator; run explicitly when the reference payloads change"]
fn regenerate_health_plane_vectors() {
    for kind in Kind::ALL {
        let encoded = reference_message(kind);
        let canonical_bytes = &encoded[..encoded.len() - SIGNATURE_BYTES];
        println!("[[vectors]]");
        println!("kind = \"{}\"", kind.wire());
        println!("canonical_bytes = {}", canonical_bytes.len());
        println!("canonical_hex = \"{}\"", hex(canonical_bytes));
        println!(
            "signature_hex = \"{}\"",
            hex(&encoded[encoded.len() - SIGNATURE_BYTES..])
        );
        println!();
    }
    println!("[worst_case_canonical_bytes]");
    for kind in Kind::ALL {
        let (canonical_bytes, _) = sign_envelope(
            reference_scalar(kind),
            kind.wire(),
            SESSION_ID_HEX,
            &nonce_hex(0x01),
            worst_case_payload(kind),
            MAX_SAFE_INTEGER,
        );
        println!("{} = {}", kind.wire(), canonical_bytes.len());
    }
}
