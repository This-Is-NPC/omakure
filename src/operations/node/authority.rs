use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::errors::map_identity_error;
use super::require_confirmation;
use super::status::load_node_config;
use crate::enrollment::EnrollmentRole;
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::util::hex;
use serde::{Deserialize, Serialize};

/// What this node needs to be told to mint one bundle.
///
/// The *subject* is always this node: an authority issues membership in its own
/// fleet, and letting a caller name an arbitrary subject would turn the verb
/// into a way to introduce a third party the operator never chose.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BundleIssueRequest {
    /// The node that will apply this bundle. Checked against its own identity
    /// when it does, so a bundle is useless anywhere else.
    pub audience_node_id: String,
    /// `conductor` or `performer` — the role the audience will record for this
    /// node, using the shipped integer encoding.
    pub role: String,
    pub capabilities: Vec<String>,
    /// Validity window, in seconds from now. Bounded by the frozen
    /// `BUNDLE_MAX_LIFETIME_SECONDS`.
    pub lifetime_seconds: u64,
}

/// The public half of the authority this node holds.
///
/// Deliberately has no field for the private key. There is no read path that
/// returns it, and the type is what makes that visible rather than promised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicAuthority {
    pub key_id: String,
    pub public_key: String,
}

/// A minted bundle, plus the two values the audience's `node.toml` must carry
/// for it to be accepted at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssuedBundle {
    pub bundle_hex: String,
    pub audience_node_id: String,
    pub subject_node_id: String,
    pub authority: PublicAuthority,
    pub organization: String,
    pub expires_at: u64,
}

/// Create this node's enrollment authority.
///
/// Refuses to replace an existing one. Rotating an authority key invalidates
/// every bundle it ever signed and every `trust.authorities` entry naming it,
/// on every machine in the fleet.
pub fn create_enrollment_authority(
    context: &NodeContext,
    confirmed: bool,
) -> OperationResult<PublicAuthority> {
    require_confirmation(confirmed)?;
    let authority = crate::enrollment_authority::EnrollmentAuthority::create(context)
        .map_err(map_authority_error)?;
    Ok(public_authority(&authority))
}

/// Report the authority this node holds, without its private half.
pub fn read_enrollment_authority(context: &NodeContext) -> OperationResult<PublicAuthority> {
    let authority = crate::enrollment_authority::EnrollmentAuthority::load_existing(context)
        .map_err(map_authority_error)?;
    Ok(public_authority(&authority))
}

/// Mint one enrollment bundle naming this node as the subject.
///
/// This is the shipped caller `sign_with_material` never had. Everything about
/// the signing construction is unchanged; what is new is that a fleet can now
/// perform it at all.
pub fn issue_enrollment_bundle(
    context: &NodeContext,
    request: BundleIssueRequest,
) -> OperationResult<IssuedBundle> {
    // The same shape the registry and config accept, checked here so a
    // malformed audience fails at the verb rather than inside the signer.
    if !crate::domain::is_node_id(&request.audience_node_id) {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "audience node ID is invalid",
        ));
    }
    let Some(role) = EnrollmentRole::from_wire(&request.role) else {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!(
                "role `{}` is invalid; expected conductor or performer",
                request.role
            ),
        ));
    };
    if request.lifetime_seconds == 0
        || request.lifetime_seconds > crate::enrollment::BUNDLE_MAX_LIFETIME_SECONDS
    {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!(
                "lifetime must be between 1 and {} seconds",
                crate::enrollment::BUNDLE_MAX_LIFETIME_SECONDS
            ),
        ));
    }

    let config = load_node_config(context)?;
    if config.organization.id.is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "this node has no organization; a bundle without one cannot be verified",
        ));
    }

    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    let transport = crate::node_transport::LocalTransport::load_existing(context, &identity)
        .map_err(|error| {
            OperationError::new(OperationErrorCode::RegistryInvalid, error.to_string())
        })?;
    let certificate = transport.certificate();

    let authority = crate::enrollment_authority::EnrollmentAuthority::load_existing(context)
        .map_err(map_authority_error)?;

    let mut bundle_id = [0u8; crate::enrollment::REQUEST_ID_BYTES];
    crate::util::entropy::fill_bytes(&mut bundle_id);
    let issued_at = crate::util::time::unix_seconds();
    let expires_at = issued_at.saturating_add(request.lifetime_seconds);

    let subject_node_id = identity.public_status().node_id.clone();
    let bundle = authority
        .issue(
            bundle_id,
            config.organization.id.clone(),
            request.audience_node_id.clone(),
            subject_node_id.clone(),
            *certificate.identity_key(),
            *certificate.transport_public(),
            *certificate.as_bytes(),
            role,
            request.capabilities,
            issued_at,
            expires_at,
        )
        .map_err(map_authority_error)?;

    Ok(IssuedBundle {
        bundle_hex: hex::encode(&bundle),
        audience_node_id: request.audience_node_id,
        subject_node_id,
        authority: public_authority(&authority),
        organization: config.organization.id,
        expires_at,
    })
}

fn public_authority(
    authority: &crate::enrollment_authority::EnrollmentAuthority,
) -> PublicAuthority {
    PublicAuthority {
        key_id: hex::encode(&authority.key_id()),
        public_key: hex::encode(&authority.public_key()),
    }
}

fn map_authority_error(error: crate::enrollment_authority::AuthorityError) -> OperationError {
    OperationError::new(OperationErrorCode::RegistryInvalid, error.to_string())
}
