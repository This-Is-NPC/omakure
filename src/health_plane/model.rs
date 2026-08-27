//! Stable Health Plane error codes, message kinds, and protocol-neutral
//! domain types.
//!
//! Nothing in this module reads or writes storage, and nothing here is
//! transport-aware.  The types are the single vocabulary shared by ingest,
//! current-state queries, fleet-status derivation, and the bounded Signal
//! inbox and outbox.

use super::bounds::{
    CAPABILITY_PROFILE_PULSE, CAPABILITY_SIGNAL, MAX_CANONICAL_ACK, MAX_CANONICAL_ERROR,
    MAX_CANONICAL_PROFILE, MAX_CANONICAL_PULSE, MAX_CANONICAL_SIGNAL, MAX_STORED_PROFILE_BYTES,
    MAX_STORED_PULSE_BYTES, MAX_STORED_SIGNAL_BYTES, PRESENCE_ONLINE_SECONDS,
    PRESENCE_STALE_SECONDS, ROLE_CONDUCTOR, ROLE_PERFORMER, SIGNATURE_BYTES,
};
use serde::{Deserialize, Serialize};

/// The frozen Health Plane rejection codes, `1101..=1115`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthCode {
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
    /// Every frozen code, in ascending numeric order.
    pub const ALL: [HealthCode; 15] = [
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

    /// The numeric code carried in `health_error.code` and in audit rows.
    pub const fn code(self) -> u16 {
        self as u16
    }

    /// The stable snake_case name carried in `health_error.reason`.
    pub const fn name(self) -> &'static str {
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

    /// Resolve a numeric code back to its stable variant.
    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|entry| entry.code() == code)
    }
}

/// The closed set of five Health Plane message kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthKind {
    Profile,
    Pulse,
    Signal,
    Ack,
    Error,
}

impl HealthKind {
    /// Every frozen kind.
    pub const ALL: [HealthKind; 5] = [
        HealthKind::Profile,
        HealthKind::Pulse,
        HealthKind::Signal,
        HealthKind::Ack,
        HealthKind::Error,
    ];

    /// The envelope `kind` string.
    pub const fn wire(self) -> &'static str {
        match self {
            HealthKind::Profile => "health_profile",
            HealthKind::Pulse => "health_pulse",
            HealthKind::Signal => "health_signal",
            HealthKind::Ack => "health_ack",
            HealthKind::Error => "health_error",
        }
    }

    /// The single payload body field this kind carries.
    pub const fn body_field(self) -> &'static str {
        match self {
            HealthKind::Profile => "profile",
            HealthKind::Pulse => "pulse",
            HealthKind::Signal => "signal",
            HealthKind::Ack => "ack",
            HealthKind::Error => "error",
        }
    }

    /// The per-kind canonical byte cap, excluding the signature.
    pub const fn max_canonical_bytes(self) -> usize {
        match self {
            HealthKind::Profile => MAX_CANONICAL_PROFILE,
            HealthKind::Pulse => MAX_CANONICAL_PULSE,
            HealthKind::Signal => MAX_CANONICAL_SIGNAL,
            HealthKind::Ack => MAX_CANONICAL_ACK,
            HealthKind::Error => MAX_CANONICAL_ERROR,
        }
    }

    /// The per-kind encoded byte cap, canonical bytes plus the signature.
    pub const fn max_encoded_bytes(self) -> usize {
        self.max_canonical_bytes() + SIGNATURE_BYTES
    }

    /// The maximum bytes a single stored row for this kind may account for.
    pub const fn max_stored_bytes(self) -> Option<i64> {
        match self {
            HealthKind::Profile => Some(MAX_STORED_PROFILE_BYTES),
            HealthKind::Pulse => Some(MAX_STORED_PULSE_BYTES),
            HealthKind::Signal => Some(MAX_STORED_SIGNAL_BYTES),
            HealthKind::Ack | HealthKind::Error => None,
        }
    }

    /// The `trusted_peers.role` the sending peer must hold on the receiver.
    pub const fn required_role(self) -> i64 {
        match self {
            HealthKind::Profile | HealthKind::Pulse | HealthKind::Signal => ROLE_PERFORMER,
            HealthKind::Ack | HealthKind::Error => ROLE_CONDUCTOR,
        }
    }

    /// The capability the sending peer must have been granted, if any.
    pub const fn required_capability(self) -> Option<&'static str> {
        match self {
            HealthKind::Profile | HealthKind::Pulse => Some(CAPABILITY_PROFILE_PULSE),
            HealthKind::Signal => Some(CAPABILITY_SIGNAL),
            HealthKind::Ack | HealthKind::Error => None,
        }
    }

    /// Parse an envelope `kind` string into a frozen kind.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.wire() == value)
    }
}

/// The closed lifecycle Signal kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignalKind {
    Enrolled,
    Revoked,
    RunCompleted,
}

impl SignalKind {
    /// Every frozen Signal kind.
    pub const ALL: [SignalKind; 3] = [
        SignalKind::Enrolled,
        SignalKind::Revoked,
        SignalKind::RunCompleted,
    ];

    /// The wire spelling of the Signal kind.
    pub const fn wire(self) -> &'static str {
        match self {
            SignalKind::Enrolled => "enrolled",
            SignalKind::Revoked => "revoked",
            SignalKind::RunCompleted => "run-completed",
        }
    }

    /// Parse a wire Signal kind.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.wire() == value)
    }
}

/// One runtime fact reported by a Profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFact {
    pub available: bool,
    pub name: String,
    pub version: String,
}

/// The latest static node facts for one Performer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileSnapshot {
    pub agent_version: String,
    pub arch: String,
    /// The derived name of the baseline this node recorded installing, or empty
    /// when it holds none. The claim.
    pub baseline_id: String,
    /// The same derivation recomputed over the paths that baseline named, as
    /// they are on this node's disk now, or empty when it holds none. The
    /// evidence. Comparing the two is the whole of drift; neither field on its
    /// own is a verdict.
    pub baseline_observed_id: String,
    pub capabilities: Vec<String>,
    pub display_name: String,
    pub distro_id: String,
    pub distro_version: String,
    pub omarchy_channel: String,
    pub omarchy_version: String,
    pub platform: String,
    pub profile_revision: u64,
    pub role: String,
    pub runtimes: Vec<RuntimeFact>,
}

/// Bounded facts about one terminal run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFact {
    pub exit_code: Option<i64>,
    pub finished_at: i64,
    pub run_id: String,
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
}

/// Runner and scheduler liveness carried by a Pulse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnerFact {
    pub queue_depth: u64,
    pub scheduler: String,
    pub state: String,
    pub workers_busy: u64,
    pub workers_configured: u64,
}

/// The latest liveness snapshot for one Performer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PulseSnapshot {
    pub emitted_at: i64,
    pub last_run: Option<RunFact>,
    pub profile_revision: u64,
    pub runner: RunnerFact,
    pub sequence: u64,
    pub uptime_seconds: u64,
}

/// One closed-lifecycle Signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalRecord {
    pub kind: SignalKind,
    pub occurred_at: i64,
    pub run: Option<RunFact>,
    pub sequence: u64,
    pub signal_id: String,
    pub subject: Option<String>,
}

/// A positive acknowledgement carrying the receiver's Signal cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AckBody {
    pub accepted: bool,
    pub acked_message_id: String,
    pub cursor: u64,
}

/// A bounded rejection carrying only a stable code and its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorBody {
    pub accepted: bool,
    pub acked_message_id: String,
    pub code: u16,
    pub reason: String,
}

/// The validated body of one Health Plane message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthBody {
    Profile(ProfileSnapshot),
    Pulse(PulseSnapshot),
    Signal(SignalRecord),
    Ack(AckBody),
    Error(ErrorBody),
}

impl HealthBody {
    /// The kind this body belongs to.
    pub const fn kind(&self) -> HealthKind {
        match self {
            HealthBody::Profile(_) => HealthKind::Profile,
            HealthBody::Pulse(_) => HealthKind::Pulse,
            HealthBody::Signal(_) => HealthKind::Signal,
            HealthBody::Ack(_) => HealthKind::Ack,
            HealthBody::Error(_) => HealthKind::Error,
        }
    }
}

/// A fully validated Health Plane payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthPayload {
    pub message_id: String,
    pub target: String,
    pub body: HealthBody,
}

/// Conductor-local presence, derived from the last accepted Pulse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Unknown,
    Online,
    Stale,
    Offline,
}

impl Presence {
    /// The stable projection name.
    pub const fn name(self) -> &'static str {
        match self {
            Presence::Unknown => "unknown",
            Presence::Online => "online",
            Presence::Stale => "stale",
            Presence::Offline => "offline",
        }
    }

    /// Derive presence from the age in seconds of the last accepted Pulse.
    pub fn derive(last_pulse_at: Option<i64>, now: i64) -> Self {
        let Some(last_pulse_at) = last_pulse_at else {
            return Presence::Unknown;
        };
        let age = now.saturating_sub(last_pulse_at);
        if age <= PRESENCE_ONLINE_SECONDS {
            Presence::Online
        } else if age <= PRESENCE_STALE_SECONDS {
            Presence::Stale
        } else {
            Presence::Offline
        }
    }
}

/// The outcome of a fully evaluated inbound Health Plane message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthDecision {
    /// Applied in one transaction; the reply is a `health_ack` with `cursor`.
    Accepted { cursor: u64 },
    /// Stored in the bounded reorder buffer; the acknowledged cursor is
    /// unchanged and the Performer resends from `cursor + 1`.
    Held { cursor: u64 },
    /// Rejected with a stable code; no health state was written.
    Rejected(HealthCode),
}

impl HealthDecision {
    /// The stable code when the decision is a rejection.
    pub const fn code(&self) -> Option<HealthCode> {
        match self {
            HealthDecision::Rejected(code) => Some(*code),
            _ => None,
        }
    }

    /// The audit outcome name for this decision.
    pub const fn outcome(&self) -> &'static str {
        match self {
            HealthDecision::Accepted { .. } => "accepted",
            HealthDecision::Held { .. } => "held",
            HealthDecision::Rejected(_) => "rejected",
        }
    }
}
