use crate::enrollment::{self};
use crate::node::{NodeContext, NodePathOverrides};

use super::{OperationError, OperationErrorCode, OperationResult};

mod authority;
mod bundle;
mod discovery;
mod errors;
mod manual_enrollment;
mod status;
mod trust;

pub use authority::{
    BundleIssueRequest, IssuedBundle, PublicAuthority, create_enrollment_authority,
    issue_enrollment_bundle, read_enrollment_authority,
};
pub use bundle::{
    BOOTSTRAP_TOKEN_FILE_ENV, SignedBundleApplyRequest, apply_signed_bundle,
    apply_signed_bundle_authenticated, apply_signed_bundle_from_local_token,
    recover_local_bootstrap_token_tombstones, signed_bundle_enrollment_enabled,
};
pub use discovery::{public_discovery_status, public_discovery_status_with_config, scan_discovery};
pub(crate) use errors::{map_direct_service_error, map_node_error, map_registry_error};
pub use manual_enrollment::{
    ManualEnrollmentApprovalRequest, ManualEnrollmentRejectionRequest, ManualEnrollmentResult,
    approve_manual_enrollment, list_pending_enrollments, manual_enrollment_enabled,
    reject_manual_enrollment, request_manual_enrollment, stage_manual_enrollment,
    stage_manual_enrollment_hex,
};
pub(crate) use status::initialize_node_locked;
pub(crate) use status::open_observational_registry;
pub use status::{
    NodeInitializationResult, NodeResetResult, NodeStatus, PublicIdentity, PublicNodeConfig,
    TrustSummary, initialize_node, initialize_node_nonblocking, load_node_config,
    open_registry_for_baseline, public_node_status, reset_node,
};
pub use trust::{
    CapabilityUpdateRequest, ManualTrustRequest, PublicPeer, RevocationRequest,
    import_manual_trust, list_trusted_peers, reconcile_revoked_cue_runs, revoke_peer,
    update_peer_capabilities,
};

pub fn resolve_context(overrides: NodePathOverrides) -> OperationResult<NodeContext> {
    NodeContext::resolve(overrides).map_err(map_node_error)
}

fn decode_fixed_hex(value: &str, bytes: usize, label: &str) -> OperationResult<Vec<u8>> {
    enrollment::parse_hex(value, bytes).map_err(|_| {
        OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            format!("{label} must be lowercase hexadecimal bytes"),
        )
    })
}

fn require_confirmation(confirmed: bool) -> OperationResult<()> {
    if confirmed {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "explicit confirmation is required for trust mutation",
        ))
    }
}

#[cfg(test)]
mod tests;
