use super::error::RegistryError;
use crate::domain::health_plane::bounds::{ROLE_CONDUCTOR, ROLE_PERFORMER};
use crate::domain::health_plane::model::{LifecycleState, LifecycleTransition};
use crate::enrollment::EnrollmentRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRole {
    Conductor,
    Performer,
}

impl PeerRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conductor => "conductor",
            Self::Performer => "performer",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        EnrollmentRole::from_wire(value).map(Self::from)
    }

    pub(super) fn parse(value: &str) -> Result<Self, RegistryError> {
        Self::from_wire(value)
            .ok_or_else(|| RegistryError::InvalidSchema(format!("unknown peer role {value:?}")))
    }

    /// The integer stored in `trusted_peers.role` and required by Health
    /// Plane frames.
    pub const fn code(self) -> i64 {
        match self {
            Self::Conductor => ROLE_CONDUCTOR,
            Self::Performer => ROLE_PERFORMER,
        }
    }

    pub fn from_code(code: i64) -> Option<Self> {
        match code {
            ROLE_CONDUCTOR => Some(Self::Conductor),
            ROLE_PERFORMER => Some(Self::Performer),
            _ => None,
        }
    }
}

impl From<EnrollmentRole> for PeerRole {
    fn from(role: EnrollmentRole) -> Self {
        match role {
            EnrollmentRole::Conductor => Self::Conductor,
            EnrollmentRole::Performer => Self::Performer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    Pending,
    Active,
    Suspended,
    Revoked,
}

impl PeerState {
    fn lifecycle_state(self) -> LifecycleState {
        match self {
            Self::Active => LifecycleState::Active,
            Self::Revoked => LifecycleState::Revoked,
            Self::Pending | Self::Suspended => LifecycleState::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Revoked => "revoked",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, RegistryError> {
        match value {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "revoked" => Ok(Self::Revoked),
            _ => Err(RegistryError::InvalidSchema(format!(
                "unknown peer state {value:?}"
            ))),
        }
    }
}

impl std::fmt::Display for PeerState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    Manual,
    Bundle,
    Recovery,
}

impl PeerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Bundle => "bundle",
            Self::Recovery => "recovery",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, RegistryError> {
        match value {
            "manual" => Ok(Self::Manual),
            "bundle" => Ok(Self::Bundle),
            "recovery" => Ok(Self::Recovery),
            _ => Err(RegistryError::InvalidSchema(format!(
                "unknown peer source {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRegistration {
    pub node_id: String,
    pub public_key: String,
    pub role: PeerRole,
    pub capabilities: Vec<String>,
    pub source: PeerSource,
    pub actor: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRecord {
    pub node_id: String,
    pub public_key: String,
    pub role: PeerRole,
    pub state: PeerState,
    pub capabilities: Vec<String>,
    pub added_at: String,
    pub updated_at: String,
    pub last_seen: Option<String>,
    pub source: PeerSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportPeer {
    pub node_id: String,
    pub identity_key: [u8; 32],
    pub transport_public_key: Option<[u8; 32]>,
    pub key_epoch: Option<u64>,
    pub state: PeerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCounts {
    pub total: usize,
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationRecord {
    pub id: i64,
    pub node_id: String,
    pub public_key: String,
    pub revoked_at: String,
    pub reason: String,
    pub replacement_node_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub id: i64,
    pub event_type: String,
    pub node_id: String,
    pub from_state: Option<PeerState>,
    pub to_state: Option<PeerState>,
    pub actor: String,
    pub reason: String,
    pub occurred_at: String,
}

impl AuditEvent {
    pub(crate) fn lifecycle_transition(&self) -> LifecycleTransition<'_> {
        LifecycleTransition {
            id: self.id,
            node_id: &self.node_id,
            from_state: self.from_state.map(PeerState::lifecycle_state),
            to_state: self.to_state.map(PeerState::lifecycle_state),
            occurred_at: chrono::DateTime::parse_from_rfc3339(&self.occurred_at)
                .ok()
                .map(|time| time.timestamp()),
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn audit_transition_view_excludes_private_fields_and_preserves_timestamp_validation() {
        let mut audit = AuditEvent {
            id: 7,
            event_type: "peer_transition".into(),
            node_id: format!("omk1_{}", "a".repeat(64)),
            from_state: Some(PeerState::Pending),
            to_state: Some(PeerState::Active),
            actor: "/home/operator/secret-path".into(),
            reason: "secret://vault/token AWS_SECRET=abc".into(),
            occurred_at: "2023-11-14T22:13:20Z".into(),
        };
        let view = audit.lifecycle_transition();
        assert_eq!(view.from_state, Some(LifecycleState::Other));
        assert_eq!(view.to_state, Some(LifecycleState::Active));
        assert_eq!(view.occurred_at, Some(1_700_000_000));
        assert_eq!(view.id, 7);
        assert_eq!(view.node_id, audit.node_id);

        audit.occurred_at = "invalid".into();
        let invalid = audit.lifecycle_transition();
        assert_eq!(invalid.occurred_at, None);
    }
}
