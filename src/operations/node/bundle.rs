use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::decode_fixed_hex;
use super::errors::{map_enrollment_error, map_identity_error, map_node_error, map_registry_error};
use super::status::{initialize_node_nonblocking, load_node_config, path_is_present};
use super::trust::{public_peer, PublicPeer};
use crate::domain::NodeConfig;
use crate::enrollment::{self};
use crate::node::{
    NodeContext, PrivateFileCommitStatus, PrivateTokenLease, DATABASE_FILE, IDENTITY_KEY_FILE,
};
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::util::hex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::PathBuf;
use subtle::ConstantTimeEq;

const SIGNED_BUNDLE_ACTOR: &str = "signed-bundle-installer";

const SIGNED_BUNDLE_REASON: &str = "unattended signed enrollment bundle";

/// Environment variable naming the token file `node serve` bootstraps from.
pub const BOOTSTRAP_TOKEN_FILE_ENV: &str = "OMAKURE_BOOTSTRAP_TOKEN_FILE";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SignedBundleApplyRequest {
    pub bundle_hex: String,
    pub bootstrap_token: String,
    pub bootstrap_nonce: String,
    pub bootstrap_token_path: Option<PathBuf>,
}

pub fn signed_bundle_enrollment_enabled(context: &NodeContext) -> OperationResult<NodeConfig> {
    let config = load_node_config(context)?;
    if config.trust.enrollment != "signed-bundle" {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentDisabled,
            "signed-bundle enrollment is not enabled",
        ));
    }
    Ok(config)
}

pub fn apply_signed_bundle(
    context: &NodeContext,
    request: SignedBundleApplyRequest,
) -> OperationResult<PublicPeer> {
    apply_signed_bundle_with_actor(context, request, SIGNED_BUNDLE_ACTOR)
}

pub fn apply_signed_bundle_authenticated(
    context: &NodeContext,
    request: SignedBundleApplyRequest,
    token_id: &str,
) -> OperationResult<PublicPeer> {
    let actor = format!("auth-token:{token_id}");
    apply_signed_bundle_with_actor(context, request, &actor)
}

fn apply_signed_bundle_with_actor(
    context: &NodeContext,
    mut request: SignedBundleApplyRequest,
    actor: &str,
) -> OperationResult<PublicPeer> {
    let config = signed_bundle_enrollment_enabled(context)?;
    let nonce = decode_fixed_hex(&request.bootstrap_nonce, 16, "bootstrap nonce")?;
    let state_ready = context
        .validate_existing_state_contents()
        .map_err(map_node_error)?;
    let identity_ready = path_is_present(&context.identity_path(), IDENTITY_KEY_FILE)?;
    let registry_ready = path_is_present(&context.database_path(), DATABASE_FILE)?;
    if !state_ready || !identity_ready || !registry_ready {
        let _ = initialize_node_nonblocking(context, &config)?;
    }
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())
        .map_err(map_registry_error)?;
    let mut token_lease = if let Some(path) = request.bootstrap_token_path.as_deref() {
        recover_private_token_tombstones(context, &registry, &config.organization.id, path)?;
        let lease = context
            .stage_private_bounded_file(path, enrollment::MAX_BOOTSTRAP_TOKEN_BYTES)
            .map_err(map_node_error)?;
        let token = match String::from_utf8(lease.contents().to_vec()) {
            Ok(token) => token,
            Err(_) => {
                let mut lease = Some(lease);
                return Err(restore_token_lease(
                    &mut lease,
                    OperationError::new(
                        OperationErrorCode::EnrollmentDenied,
                        "bootstrap token is invalid",
                    ),
                ));
            }
        };
        request.bootstrap_token = token.trim().to_string();
        Some(lease)
    } else {
        None
    };
    if request.bootstrap_token.len() < 32
        || request.bootstrap_token.len() > enrollment::MAX_BOOTSTRAP_TOKEN_BYTES
        || request
            .bootstrap_token
            .bytes()
            .any(|byte| byte.is_ascii_control())
    {
        return Err(restore_token_lease(
            &mut token_lease,
            OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "bootstrap token is invalid",
            ),
        ));
    }
    let bundle_bytes = match decode_bundle(&request.bundle_hex) {
        Ok(bytes) => bytes,
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                None,
                error,
            )
        }
    };
    let bundle = match enrollment::SignedEnrollmentBundle::decode(&bundle_bytes) {
        Ok(bundle) => bundle,
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                None,
                map_enrollment_error(error),
            )
        }
    };
    let preflight = (|| {
        let authority = config
            .trust
            .authorities
            .iter()
            .find(|authority| authority.key_id == hex::encode(&bundle.authority_key_id))
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::EnrollmentInvalid,
                    "signed enrollment authority is not configured",
                )
            })?;
        let authority = enrollment::BundleAuthority {
            key_id: enrollment::parse_hex(&authority.key_id, 16)
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority key ID is invalid",
                    )
                })?
                .try_into()
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority key ID is invalid",
                    )
                })?,
            public_key: enrollment::parse_hex(&authority.public_key, 32)
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority public key is invalid",
                    )
                })?
                .try_into()
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority public key is invalid",
                    )
                })?,
            revoked: authority.revoked,
        };
        let now = crate::util::time::unix_seconds();
        bundle
            .verify(
                &authority,
                &config.organization.id,
                identity.public_status().node_id.as_str(),
                now,
            )
            .map_err(map_enrollment_error)?;
        let certificate =
            crate::direct_transport::TransportCertificate::from_bytes(&bundle.subject_certificate)
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "signed enrollment certificate is invalid",
                    )
                })?;
        certificate.verify_time(now).map_err(|_| {
            OperationError::new(
                OperationErrorCode::EnrollmentExpired,
                "signed enrollment certificate is expired",
            )
        })?;
        let token_hash = enrollment::hash_bootstrap_token(request.bootstrap_token.as_bytes());
        let nonce_hash = enrollment::hash_bootstrap_nonce(&nonce);
        if hex::encode(&token_hash)
            .as_bytes()
            .ct_eq(config.trust.bootstrap_token_hash.as_bytes())
            .unwrap_u8()
            != 1
            || hex::encode(&nonce_hash)
                .as_bytes()
                .ct_eq(config.trust.bootstrap_nonce_hash.as_bytes())
                .unwrap_u8()
                != 1
        {
            return Err(OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "bootstrap proof does not match local policy",
            ));
        }
        Ok((now, token_hash, nonce_hash))
    })();
    let (now, token_hash, nonce_hash) = match preflight {
        Ok(value) => value,
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                Some(&bundle),
                error,
            )
        }
    };
    let mut peer = match registry
        .activate_signed_bundle(
            &bundle,
            actor,
            SIGNED_BUNDLE_REASON,
            now,
            &token_hash,
            &nonce_hash,
        )
        .map_err(map_registry_error)
    {
        Ok(peer) => public_peer(peer),
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                Some(&bundle),
                error,
            )
        }
    };
    if let Some(lease) = token_lease.take() {
        let bundle_digest: [u8; 32] = Sha256::digest(bundle.encode()).into();
        peer.cleanup_pending = !complete_token_cleanup(
            &registry,
            lease,
            &config.organization.id,
            &token_hash,
            &nonce_hash,
            &bundle.bundle_id,
            &bundle_digest,
        );
    }
    Ok(peer)
}

pub fn apply_signed_bundle_from_local_token(
    context: &NodeContext,
    mut request: SignedBundleApplyRequest,
    token_id: &str,
) -> OperationResult<PublicPeer> {
    let token_path = std::env::var_os(BOOTSTRAP_TOKEN_FILE_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "local bootstrap token file is not configured",
            )
        })?;
    request.bootstrap_token.clear();
    request.bootstrap_token_path = Some(token_path);
    apply_signed_bundle_authenticated(context, request, token_id)
}

fn restore_token_lease(
    lease: &mut Option<PrivateTokenLease>,
    error: OperationError,
) -> OperationError {
    if let Some(lease) = lease.take() {
        if lease.restore().is_err() {
            return OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "bootstrap token could not be restored",
            );
        }
    }
    error
}

fn record_signed_bundle_failure_with_token(
    registry: &NodeRegistry,
    lease: &mut Option<PrivateTokenLease>,
    bundle: Option<&enrollment::SignedEnrollmentBundle>,
    error: OperationError,
) -> OperationResult<PublicPeer> {
    record_signed_bundle_failure(registry, bundle, restore_token_lease(lease, error))
}

pub(super) fn recover_private_token_tombstones(
    context: &NodeContext,
    registry: &NodeRegistry,
    organization: &str,
    path: &std::path::Path,
) -> OperationResult<()> {
    let _lock = context
        .acquire_private_token_lock(path)
        .map_err(map_node_error)?;
    let pending = registry
        .pending_bootstrap_cleanups(
            organization,
            crate::node::PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT,
        )
        .map_err(map_registry_error)?;
    let mut completed = vec![false; pending.len()];
    for lease in context
        .list_private_token_tombstones(path, enrollment::MAX_BOOTSTRAP_TOKEN_BYTES)
        .map_err(map_node_error)?
    {
        let token_hash = enrollment::hash_bootstrap_token(lease.contents());
        if let Some((index, cleanup)) = pending
            .iter()
            .enumerate()
            .find(|(_, cleanup)| cleanup.token_hash == token_hash)
        {
            if lease.finish_success() == PrivateFileCommitStatus::CleanupRequired {
                return Err(cleanup_recovery_error(&OperationError::new(
                    OperationErrorCode::IoFailed,
                    "the spent bootstrap token could not be removed",
                )));
            }
            registry
                .complete_bootstrap_cleanup(cleanup, None)
                .map_err(|error| cleanup_recovery_error(&map_registry_error(error)))?;
            completed[index] = true;
        } else if registry
            .bootstrap_proof_consumed(organization)
            .map_err(|error| cleanup_recovery_error(&map_registry_error(error)))?
        {
            if lease.finish_success() == PrivateFileCommitStatus::CleanupRequired {
                return Err(cleanup_recovery_error(&OperationError::new(
                    OperationErrorCode::IoFailed,
                    "a bootstrap token already consumed elsewhere could not be removed",
                )));
            }
        } else {
            lease.restore().map_err(|error| {
                cleanup_recovery_error(&OperationError::new(
                    OperationErrorCode::IoFailed,
                    error.to_string(),
                ))
            })?;
        }
    }
    for (index, cleanup) in pending.iter().enumerate() {
        if completed[index] {
            continue;
        }
        match fs::symlink_metadata(path) {
            Ok(_) => {
                return Err(cleanup_recovery_error(&OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!(
                        "a spent bootstrap token is still present at {}",
                        path.display()
                    ),
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(cleanup_recovery_error(&OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("{} could not be inspected: {error}", path.display()),
                )))
            }
        }
        registry
            .complete_bootstrap_cleanup(cleanup, None)
            .map_err(|error| cleanup_recovery_error(&map_registry_error(error)))?;
    }
    Ok(())
}

pub fn recover_local_bootstrap_token_tombstones(context: &NodeContext) -> OperationResult<()> {
    let Some(path) = std::env::var_os(BOOTSTRAP_TOKEN_FILE_ENV).map(PathBuf::from) else {
        return Ok(());
    };
    let result = (|| {
        let config = signed_bundle_enrollment_enabled(context)?;
        let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
        let registry = NodeRegistry::open_existing(context, identity.public_status())
            .map_err(map_registry_error)?;
        recover_private_token_tombstones(context, &registry, &config.organization.id, &path)
    })();
    result.map_err(|error| cleanup_recovery_error(&error))
}

fn complete_token_cleanup(
    registry: &NodeRegistry,
    lease: PrivateTokenLease,
    organization: &str,
    token_hash: &[u8; 32],
    nonce_hash: &[u8; 32],
    bundle_id: &[u8; 16],
    bundle_digest: &[u8; 32],
) -> bool {
    if lease.finish_success() == PrivateFileCommitStatus::CleanupRequired {
        return false;
    }
    let cleanup = crate::node_registry::PendingBootstrapCleanup {
        organization: organization.to_string(),
        token_hash: *token_hash,
        nonce_hash: *nonce_hash,
        bundle_id: *bundle_id,
    };
    match registry.complete_bootstrap_cleanup(&cleanup, Some(bundle_digest)) {
        Ok(()) => true,
        Err(_) => false,
    }
}

/// Abort startup over a failed tombstone recovery, saying what failed.
///
/// The cause used to be discarded by `map_err(|_| ...)`. That cost a real
/// debugging session: a node refused to start on a provisioned machine and the
/// message named neither the check that failed nor the file it read, while the
/// actual reason -- enrollment was not set to `signed-bundle` -- was sitting in
/// the error being thrown away. An abort message that omits its own cause is
/// the operator's whole picture.
pub(super) fn cleanup_recovery_error(cause: &OperationError) -> OperationError {
    OperationError::new(
        OperationErrorCode::IoFailed,
        format!(
            "bootstrap token cleanup recovery failed; node service startup was aborted; \
             repair the node state and retry. Cause: {}",
            cause.message
        ),
    )
}

fn record_signed_bundle_failure(
    registry: &NodeRegistry,
    bundle: Option<&enrollment::SignedEnrollmentBundle>,
    error: OperationError,
) -> OperationResult<PublicPeer> {
    let (request_id, node_id, request_digest) = match bundle {
        Some(bundle) => (
            Some(&bundle.bundle_id),
            bundle.subject_node_id.as_str(),
            Some(Sha256::digest(bundle.encode()).into()),
        ),
        None => (None, registry.local_node_id(), None),
    };
    let event_code = match error.code {
        OperationErrorCode::EnrollmentDenied => "proof_rejected",
        OperationErrorCode::EnrollmentExpired => "expired",
        _ => "malformed",
    };
    registry
        .record_enrollment_audit(
            event_code,
            request_id,
            request_digest.as_ref(),
            node_id,
            "rejected",
            "signed enrollment bundle verification failed",
        )
        .map_err(map_registry_error)?;
    Err(error)
}

fn decode_bundle(value: &str) -> OperationResult<Vec<u8>> {
    if value.is_empty() || value.len() > enrollment::MAX_BUNDLE_BYTES * 2 {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            "signed enrollment bundle bytes are invalid",
        ));
    }
    decode_fixed_hex(value, value.len() / 2, "signed enrollment bundle")
}
