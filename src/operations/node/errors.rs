use super::super::{OperationError, OperationErrorCode};
use crate::domain::NodeConfigError;
use crate::enrollment::EnrollmentError;
use crate::node::NodeError;
use crate::node_identity::NodeIdentityError;
use crate::node_registry::RegistryError;

pub(super) fn map_enrollment_error(error: EnrollmentError) -> OperationError {
    let code = match error {
        EnrollmentError::Expired => OperationErrorCode::EnrollmentExpired,
        EnrollmentError::Replay => OperationErrorCode::EnrollmentReplay,
        EnrollmentError::IdentityMismatch => OperationErrorCode::EnrollmentMismatch,
        EnrollmentError::AuthorityUnknown => OperationErrorCode::EnrollmentInvalid,
        EnrollmentError::AuthorityRevoked => OperationErrorCode::EnrollmentDenied,
        EnrollmentError::OrganizationMismatch | EnrollmentError::AudienceMismatch => {
            OperationErrorCode::EnrollmentMismatch
        }
        EnrollmentError::Invalid | EnrollmentError::TooLarge => {
            OperationErrorCode::EnrollmentInvalid
        }
    };
    OperationError::new(code, error.to_string())
}

pub(super) fn map_direct_enrollment_error(
    error: crate::direct_service::DirectServiceError,
) -> OperationError {
    let code = match error {
        crate::direct_service::DirectServiceError::Protocol(ref error) => match error.code() {
            crate::direct_transport::ProtocolErrorCode::UnsupportedVersion => {
                OperationErrorCode::TransportUnsupportedVersion
            }
            crate::direct_transport::ProtocolErrorCode::InvalidFrame => {
                OperationErrorCode::TransportInvalidFrame
            }
            crate::direct_transport::ProtocolErrorCode::MessageTooLarge => {
                OperationErrorCode::TransportMessageTooLarge
            }
            crate::direct_transport::ProtocolErrorCode::HandshakeFailed => {
                OperationErrorCode::TransportHandshakeFailed
            }
            crate::direct_transport::ProtocolErrorCode::IdentityMismatch => {
                OperationErrorCode::TransportIdentityMismatch
            }
            crate::direct_transport::ProtocolErrorCode::NotEnrolled => {
                OperationErrorCode::TransportNotEnrolled
            }
            crate::direct_transport::ProtocolErrorCode::Revoked => {
                OperationErrorCode::TransportRevoked
            }
            crate::direct_transport::ProtocolErrorCode::Expired => {
                OperationErrorCode::TransportExpired
            }
            crate::direct_transport::ProtocolErrorCode::Replay => {
                OperationErrorCode::TransportReplay
            }
            crate::direct_transport::ProtocolErrorCode::RateLimited => {
                OperationErrorCode::TransportRateLimited
            }
            crate::direct_transport::ProtocolErrorCode::Internal => {
                OperationErrorCode::TransportInternal
            }
        },
        _ => OperationErrorCode::TransportInternal,
    };
    OperationError::new(code, error.to_string())
}

fn map_config_error(error: NodeConfigError) -> OperationError {
    OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
}

pub(crate) fn map_node_error(error: NodeError) -> OperationError {
    match error {
        NodeError::Config(error) => map_config_error(error),
        NodeError::InvalidPath { .. }
        | NodeError::TestOverrideOutsideTestMode
        | NodeError::IncompleteTestOverrides => {
            OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
        }
        // Carries the file, what was wrong with it, and the remedy. The paths
        // it names are the node's own documented defaults or a path the caller
        // supplied itself, so this discloses nothing to a caller already
        // authorized to read node state -- while an opaque string leaves an
        // operator with a 0644 node.toml no route at all to `chmod 640`.
        NodeError::InsecurePath(_) => registry_error(error.to_string()),
        // Same argument as `InsecurePath` above, applied to the rest of the
        // family. These three collapsed into one opaque sentence, which cost a
        // real debugging session on a real machine: a node refused to start,
        // the operator had root, and the message named neither the path nor
        // what was wrong with it.
        NodeError::UnsafePath(_)
        | NodeError::UnexpectedFileType(_)
        | NodeError::ExistingConfig(_) => registry_error(error.to_string()),
        NodeError::LifecycleBusy => OperationError::new(
            OperationErrorCode::Conflict,
            "node service is active; stop it before changing node state",
        ),
        NodeError::TestModeUnavailable => registry_error("node test mode is unavailable"),
        NodeError::Io(_) => OperationError::new(OperationErrorCode::IoFailed, error.to_string()),
    }
}

pub(crate) fn map_identity_error(error: NodeIdentityError) -> OperationError {
    match error {
        NodeIdentityError::Node(error) => map_node_error(error),
        NodeIdentityError::Registry(error) => map_registry_error(error),
        NodeIdentityError::InvalidKey | NodeIdentityError::State(_) => {
            registry_error("node identity state is invalid or insecure")
        }
        NodeIdentityError::Io(_) | NodeIdentityError::Signing => {
            OperationError::new(OperationErrorCode::IoFailed, error.to_string())
        }
    }
}

pub(crate) fn map_registry_error(error: RegistryError) -> OperationError {
    match error {
        RegistryError::InvalidInput(error) => {
            OperationError::new(OperationErrorCode::InvalidInput, error)
        }
        // Both are conflicts and neither is the other. "This peer is already
        // trusted" and "this peer was revoked and cannot be resurrected" want
        // opposite things from an operator -- leave it alone, or issue a new
        // identity -- and collapsing them into one arm bound the inner node id
        // as the whole message, so the refusal read as nothing but the id the
        // caller had just typed.
        RegistryError::Duplicate(node_id) => OperationError::new(
            OperationErrorCode::Conflict,
            format!("{node_id} already exists as a peer or conflicts with existing state"),
        ),
        RegistryError::Revoked(node_id) => OperationError::new(
            OperationErrorCode::Conflict,
            format!(
                "{node_id} has a retained revocation and cannot be trusted again. \
                 Revocation is durable, so re-admitting this machine means giving it a \
                 new identity."
            ),
        ),
        RegistryError::InvalidTransition { from, to } => OperationError::new(
            OperationErrorCode::Conflict,
            format!("invalid trust transition from {from} to {to}"),
        ),
        RegistryError::Unchanged(error) => OperationError::new(OperationErrorCode::Conflict, error),
        RegistryError::NotFound(error) => OperationError::new(OperationErrorCode::NotFound, error),
        RegistryError::InvalidSchema(_) | RegistryError::Corrupt(_) => {
            registry_error("node trust registry is invalid or corrupt")
        }
        RegistryError::Io(error) => {
            OperationError::new(OperationErrorCode::IoFailed, error.to_string())
        }
        RegistryError::Sqlite(_) => registry_error("node trust registry is invalid or corrupt"),
        RegistryError::Node(error) => map_node_error(error),
        RegistryError::AuditCapacity => {
            registry_error("node transport audit capacity is exhausted")
        }
        RegistryError::SelfTrust => {
            OperationError::new(OperationErrorCode::Conflict, "peer cannot trust itself")
        }
        RegistryError::EnrollmentReplay => OperationError::new(
            OperationErrorCode::EnrollmentReplay,
            "manual enrollment request was replayed",
        ),
        RegistryError::EnrollmentConflict => OperationError::new(
            OperationErrorCode::Conflict,
            "manual enrollment conflicts with existing trust state",
        ),
        RegistryError::EnrollmentCapacity => {
            registry_error("manual enrollment replay capacity is exhausted")
        }
        RegistryError::EnrollmentMismatch => OperationError::new(
            OperationErrorCode::EnrollmentMismatch,
            "manual enrollment evidence does not match staged state",
        ),
        RegistryError::BundleReplay => OperationError::new(
            OperationErrorCode::EnrollmentReplay,
            "signed enrollment bundle was replayed",
        ),
        RegistryError::BundleConflict => OperationError::new(
            OperationErrorCode::Conflict,
            "signed enrollment bundle conflicts with existing trust state",
        ),
        RegistryError::BootstrapProofConsumed => OperationError::new(
            OperationErrorCode::EnrollmentReplay,
            "signed enrollment bootstrap proof was already consumed",
        ),
        RegistryError::ConductorConflict => OperationError::new(
            OperationErrorCode::Conflict,
            "an active conductor already exists",
        ),
        RegistryError::PublisherConductorConflict => OperationError::new(
            OperationErrorCode::Conflict,
            "a baseline publisher cannot also be a conductor",
        ),
        RegistryError::BundleCapacity => {
            registry_error("signed enrollment replay capacity is exhausted")
        }
        RegistryError::BundleRateLimited => OperationError::new(
            OperationErrorCode::EnrollmentRateLimited,
            "signed enrollment bundle rate limit exceeded",
        ),
    }
}

pub(crate) fn registry_error(message: impl Into<String>) -> OperationError {
    OperationError::new(OperationErrorCode::RegistryInvalid, message)
}
