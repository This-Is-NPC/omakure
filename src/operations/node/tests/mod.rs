use super::super::{OperationError, OperationErrorCode};
use super::bundle::{cleanup_recovery_error, recover_private_token_tombstones};
use super::status::MAX_NODE_CONFIG_BYTES;
use super::*;
use crate::domain::NodeConfig;
use crate::enrollment::{self, EnrollmentRole, ManualEnrollmentRequest};
use crate::node::{
    set_private_token_fault, NodeContext, NodePathOverrides, NodePlatform, PrivateTokenFault,
};
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::node_transport::LocalTransport;
use crate::test_support::node_context;
use crate::util::hex;
use rusqlite::Connection;
use std::fs;
use std::sync::{Mutex, OnceLock};
use tempfile::TempDir;

mod bundle;
mod errors;
mod manual_enrollment;
mod status;
mod trust;

static TOKEN_FAULT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn fail_enrollment_audits(context: &NodeContext, enabled: bool) {
    let connection = Connection::open(context.database_path()).unwrap();
    if enabled {
        connection
            .execute_batch(
                "DROP TRIGGER enrollment_audits_no_update;
                     CREATE TRIGGER enrollment_audits_no_update
                     BEFORE INSERT ON enrollment_audits
                     BEGIN SELECT RAISE(ABORT, 'injected enrollment audit failure'); END;",
            )
            .unwrap();
    } else {
        connection
            .execute_batch(
                "DROP TRIGGER enrollment_audits_no_update;
                     CREATE TRIGGER enrollment_audits_no_update
                     BEFORE UPDATE ON enrollment_audits
                     BEGIN SELECT RAISE(ABORT, 'enrollment audits are append-only'); END;",
            )
            .unwrap();
    }
}

fn fail_cleanup_completion_audit(context: &NodeContext, enabled: bool) {
    let connection = Connection::open(context.database_path()).unwrap();
    if enabled {
        connection
            .execute_batch(
                "DROP TRIGGER enrollment_audits_no_update;
                     CREATE TRIGGER enrollment_audits_no_update
                     BEFORE INSERT ON enrollment_audits
                     WHEN NEW.event_code = 'cleanup_completed'
                     BEGIN SELECT RAISE(ABORT, 'injected cleanup completion audit failure'); END;",
            )
            .unwrap();
    } else {
        connection
            .execute_batch(
                "DROP TRIGGER enrollment_audits_no_update;
                     CREATE TRIGGER enrollment_audits_no_update
                     BEFORE UPDATE ON enrollment_audits
                     BEGIN SELECT RAISE(ABORT, 'enrollment audits are append-only'); END;",
            )
            .unwrap();
    }
}

fn write_secure_token(path: &std::path::Path, token: &str) {
    fs::write(path, token.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

struct SignedBundleFixture {
    _target_temp: TempDir,
    _manager_temp: TempDir,
    target: NodeContext,
    request: SignedBundleApplyRequest,
    token_path: std::path::PathBuf,
    organization: String,
}

fn signed_bundle_fixture(
    authority_private: [u8; 32],
    nonce_byte: u8,
    bundle_byte: u8,
) -> SignedBundleFixture {
    let target_temp = TempDir::new().unwrap();
    let target = node_context(target_temp.path());
    let manager_temp = TempDir::new().unwrap();
    let manager_context = node_context(manager_temp.path());
    let authority_signing_key = k256::schnorr::SigningKey::from_slice(&authority_private).unwrap();
    let token = "t".repeat(32);
    let nonce = [nonce_byte; 16];
    let mut config = NodeConfig::default();
    config.organization.id = "omakure".into();
    config.trust.enrollment = "signed-bundle".into();
    config.trust.bootstrap_token_hash =
        hex::encode(&enrollment::hash_bootstrap_token(token.as_bytes()));
    config.trust.bootstrap_nonce_hash = hex::encode(&enrollment::hash_bootstrap_nonce(&nonce));
    config.trust.authorities = vec![crate::domain::EnrollmentAuthority {
        key_id: hex::encode(&[8; 16]),
        public_key: hex::encode(&authority_signing_key.verifying_key().to_bytes()),
        revoked: false,
    }];
    initialize_node(&target, &config).unwrap();
    let manager = NodeIdentity::load_or_initialize(&manager_context).unwrap();
    let manager_transport = LocalTransport::provision_new(&manager_context, &manager).unwrap();
    let target_identity = NodeIdentity::load_existing(&target).unwrap();
    let now = crate::util::time::unix_seconds();
    let bundle = enrollment::SignedEnrollmentBundle::sign_with_material(
        &authority_private,
        [bundle_byte; enrollment::REQUEST_ID_BYTES],
        [8; enrollment::BUNDLE_AUTHORITY_ID_BYTES],
        "omakure".into(),
        target_identity.public_status().node_id.clone(),
        manager.public_status().node_id.clone(),
        enrollment::parse_hex(&manager.public_status().public_key_hex, 32)
            .unwrap()
            .try_into()
            .unwrap(),
        *manager_transport.certificate().transport_public(),
        *manager_transport.certificate().as_bytes(),
        EnrollmentRole::Conductor,
        vec!["remote-run".into()],
        now,
        now + 600,
    )
    .unwrap();
    let token_path = target_temp.path().join("bootstrap.token");
    write_secure_token(&token_path, &token);
    SignedBundleFixture {
        target,
        request: SignedBundleApplyRequest {
            bundle_hex: hex::encode(&bundle.encode()),
            bootstrap_token: token,
            bootstrap_nonce: hex::encode(&nonce),
            bootstrap_token_path: Some(token_path.clone()),
        },
        token_path,
        organization: "omakure".into(),
        _target_temp: target_temp,
        _manager_temp: manager_temp,
    }
}

fn peer_request(identity: &NodeIdentity) -> ManualTrustRequest {
    let key = k256::schnorr::SigningKey::from_slice(&[3; 32]).unwrap();
    let public_key = hex::encode(&key.verifying_key().to_bytes());
    let node_id =
        crate::node_identity::node_id_for_x_only_public_key(&key.verifying_key().to_bytes());
    assert_ne!(node_id, identity.public_status().node_id);
    ManualTrustRequest {
        node_id,
        public_key,
        transport_certificate: None,
        role: "performer".into(),
        capabilities: vec!["remote-run".into()],
        actor: "operator".into(),
        reason: "approved manually".into(),
        confirmed: true,
    }
}
