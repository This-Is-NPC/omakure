//! Every quantitative bound frozen by `.docs/health-plane-contract.md`.
//!
//! These constants are a transcription of the frozen contract and of
//! `tests/fixtures/health_plane_vectors.toml`.  They are never negotiated and
//! never widened at runtime. One — [`BASELINE_ID_HEX_CHARS`] — is derived
//! rather than transcribed, for the reason written at its definition.

/// `payload.health_version` accepted by this implementation.
pub const HEALTH_VERSION: u64 = 1;
/// Node registry schema version that owns the Health Plane tables.
pub const REGISTRY_SCHEMA_VERSION: i64 = 8;

/// Frozen trust roles as stored in `trusted_peers.role`.
pub const ROLE_CONDUCTOR: i64 = 1;
/// Frozen trust roles as stored in `trusted_peers.role`.
pub const ROLE_PERFORMER: i64 = 2;

/// Capability required by `health_profile` and `health_pulse`.
pub const CAPABILITY_PROFILE_PULSE: &str = "inventory-health";
/// Capability required by `health_signal`.
pub const CAPABILITY_SIGNAL: &str = "notifications";

/// The frozen capability allow-list shared with the transport contract.
pub const CAPABILITY_ALLOWLIST: [&str; 7] = [
    "backup-orchestration",
    "baseline-push",
    "inventory-health",
    "lost-device-revocation",
    "notifications",
    "remote-run",
    "ssh-credential-rotation",
];

/// The frozen runtime-name allow-list, in the sorted order the closed schema
/// requires.
///
/// Sender-side clamping and receiver-side validation must read the same list:
/// a Profile built from one list and validated against another would be
/// rejected on the wire with no way for either side to explain why.
pub const RUNTIME_NAMES: [&str; MAX_RUNTIME_COUNT] = ["bash", "powershell", "python", "sh"];

/// Width of a baseline identity as the Profile carries it, in hex characters.
///
/// The one constant in this file that is derived rather than transcribed, and
/// deliberately so. Every other number here is a policy choice the contract
/// froze; this one is not a choice at all — it is the width of the identity
/// `crate::baseline` computes, and a Profile that validated a different width
/// would reject an identity the baseline plane can legitimately produce. The
/// literal `64` is transcribed independently by
/// `tests/health_plane_contract.rs`, which is what would catch a change here
/// that the contract did not agree to.
pub const BASELINE_ID_HEX_CHARS: usize = crate::baseline::BASELINE_ID_BYTES * 2;

// Message size bounds, in canonical envelope bytes excluding the signature.
pub const MAX_CANONICAL_PROFILE: usize = 2_048;
pub const MAX_CANONICAL_PULSE: usize = 1_280;
pub const MAX_CANONICAL_SIGNAL: usize = 1_024;
pub const MAX_CANONICAL_ACK: usize = 768;
pub const MAX_CANONICAL_ERROR: usize = 768;
pub const SIGNATURE_BYTES: usize = 64;

// Structural bounds.
pub const MAX_JSON_DEPTH: usize = 5;
pub const MAX_FIELD_NAME_BYTES: usize = 32;
pub const MAX_PAYLOAD_FIELDS: usize = 64;
pub const MAX_ARRAY_LENGTH: usize = 32;
pub const MAX_STRING_BYTES: usize = 128;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const NODE_ID_BYTES: usize = 69;
pub const OPAQUE_ID_HEX_CHARS: usize = 32;

// Field bounds.
pub const MAX_AGENT_VERSION_BYTES: usize = 32;
pub const MAX_DISPLAY_NAME_BYTES: usize = 64;
pub const MAX_DISTRO_ID_BYTES: usize = 32;
pub const MAX_DISTRO_VERSION_BYTES: usize = 32;
pub const MAX_SCRIPT_BYTES: usize = 64;
pub const MAX_CAPABILITY_BYTES: usize = 64;
pub const MAX_CAPABILITY_COUNT: usize = 32;
pub const MAX_RUNTIME_COUNT: usize = 4;
pub const MAX_WORKERS: u64 = 255;
pub const MAX_QUEUE_DEPTH: u64 = 65_535;
pub const MAX_UPTIME_SECONDS: u64 = 4_294_967_295;
pub const MIN_EXIT_CODE: i64 = -256;
pub const MAX_EXIT_CODE: i64 = 255;

// Freshness, skew, and presence.
pub const MAX_AGE_SECONDS: i64 = 120;
pub const MAX_FUTURE_SKEW_SECONDS: i64 = 60;
pub const PRESENCE_ONLINE_SECONDS: i64 = 90;
pub const PRESENCE_STALE_SECONDS: i64 = 600;

// Rate bounds.
pub const NOMINAL_PULSE_INTERVAL_SECONDS: i64 = 30;
pub const MIN_PULSE_INTERVAL_SECONDS: i64 = 10;
pub const MAX_MESSAGES_PER_PEER_PER_MINUTE: i64 = 20;
pub const MAX_PROFILES_PER_PEER_PER_HOUR: i64 = 12;
pub const MAX_SIGNALS_PER_PEER_PER_MINUTE: i64 = 10;
pub const RATE_BURST_ALLOWANCE: i64 = 5;
pub const MAX_IN_FLIGHT_PER_SESSION: i64 = 8;
/// Fixed rate window for the per-minute counters.
pub const RATE_MINUTE_WINDOW_SECONDS: i64 = 60;
/// Fixed rate window for the per-hour Profile counter.
pub const RATE_HOUR_WINDOW_SECONDS: i64 = 3_600;

// Node-count bounds.
pub const MAX_PERFORMERS_PER_CONDUCTOR: i64 = 256;
pub const MAX_CONDUCTORS_PER_PERFORMER: i64 = 1;

// Queue, retry, and timeout bounds.
pub const SIGNAL_OUTBOX_CAPACITY: i64 = 64;
pub const SIGNAL_INBOX_CAPACITY: i64 = 64;
pub const SIGNAL_GLOBAL_INBOX_CAPACITY: i64 = 16_384;
pub const ACK_TIMEOUT_SECONDS: i64 = 5;
pub const MAX_RETRIES: i64 = 3;
pub const RETRY_BACKOFF_SECONDS: [i64; 3] = [1, 2, 4];
pub const PROCESSING_BUDGET_MILLIS: u64 = 250;

// Ordering and cursor bounds.
pub const REORDER_BUFFER_ENTRIES: u64 = 32;
pub const REORDER_BUFFER_SECONDS: i64 = 60;

// Replay bounds.
pub const REPLAY_SECURITY_FLOOR_SECONDS: i64 = 180;
pub const REPLAY_RETENTION_SECONDS: i64 = 900;
pub const MAX_REPLAY_ROWS: i64 = 131_072;
pub const REPLAY_ROW_BYTES: i64 = 32;

// Storage and retention bounds.
pub const SIGNAL_RETENTION_SECONDS: i64 = 604_800;
pub const MAX_STORED_PROFILE_BYTES: i64 = 2_112;
pub const MAX_STORED_PULSE_BYTES: i64 = 1_344;
pub const MAX_STORED_SIGNAL_BYTES: i64 = 1_088;
pub const WORST_CASE_BYTES_PER_PERFORMER: i64 = 73_088;
pub const MAX_AUDIT_ROWS: i64 = 10_000;
pub const AUDIT_ROW_BYTES: i64 = 256;
pub const AUDIT_RETENTION_SECONDS: i64 = 2_592_000;
pub const STORAGE_CEILING_BYTES: i64 = 25_464_832;

// Mixed-version policy.
pub const VERSION_INCOMPATIBLE_BACKOFF_SECONDS: i64 = 300;
pub const VERSION_INCOMPATIBLE_EXPIRY_SECONDS: i64 = 3_600;
