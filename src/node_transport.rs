//! Machine-owned direct transport key and certificate lifecycle.
//!
//! This is the filesystem adapter around the protocol-neutral transport
//! primitives. Private X25519 bytes never leave this module except through the
//! in-process Noise builder.

use crate::direct_transport::{
    unix_seconds, x25519_public_from_private, HandshakeRole, NoiseHandshake, TransportCertificate,
    TransportError, CERTIFICATE_MAX_LIFETIME_SECONDS,
};
use crate::node::{write_atomic_new, NodeContext, NodeError};
use crate::node_identity::NodeIdentity;
use rand::rngs::OsRng;
use rand::RngCore;
use std::fs;
use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeTransportError {
    #[error("transport state error: {0}")]
    State(String),
    #[error("transport node error: {0}")]
    Node(#[from] NodeError),
    #[error("transport I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("transport protocol error: {0}")]
    Protocol(#[from] TransportError),
}

pub struct LocalTransport {
    private_key: [u8; 32],
    certificate: TransportCertificate,
}

impl LocalTransport {
    pub fn provision_new(
        context: &NodeContext,
        identity: &NodeIdentity,
    ) -> Result<Self, NodeTransportError> {
        context.ensure_state_directory()?;
        let key_exists = regular_file_exists(&context.transport_key_path())?;
        let certificate_exists = regular_file_exists(&context.transport_certificate_path())?;
        if key_exists || certificate_exists {
            return Err(NodeTransportError::State(
                "transport state already exists; refusing to provision over it".to_string(),
            ));
        }
        Self::create(context, identity)
    }

    pub fn load_existing(
        context: &NodeContext,
        identity: &NodeIdentity,
    ) -> Result<Self, NodeTransportError> {
        context.ensure_state_directory()?;
        let key_exists = regular_file_exists(&context.transport_key_path())?;
        let certificate_exists = regular_file_exists(&context.transport_certificate_path())?;
        if !key_exists || !certificate_exists {
            return Err(NodeTransportError::State(
                "transport key and certificate must both exist".to_string(),
            ));
        }
        Self::load(context, identity)
    }

    pub fn certificate(&self) -> &TransportCertificate {
        &self.certificate
    }

    pub(crate) fn handshake(
        &self,
        role: HandshakeRole,
    ) -> Result<NoiseHandshake, NodeTransportError> {
        Ok(NoiseHandshake::new(
            role,
            self.private_key,
            self.certificate.clone(),
        )?)
    }

    fn create(context: &NodeContext, identity: &NodeIdentity) -> Result<Self, NodeTransportError> {
        let mut private_key = [0u8; 32];
        OsRng.fill_bytes(&mut private_key);
        let public_key = x25519_public_from_private(&private_key)?;
        let now = unix_seconds();
        let certificate = TransportCertificate::issue(
            identity,
            public_key,
            1,
            now,
            now.saturating_add(CERTIFICATE_MAX_LIFETIME_SECONDS),
            random_certificate_id(),
        )?;
        write_atomic_new(&context.transport_key_path(), &private_key, 0o600)?;
        if let Err(error) = write_atomic_new(
            &context.transport_certificate_path(),
            certificate.as_bytes(),
            0o600,
        ) {
            let _ = fs::remove_file(context.transport_key_path());
            return Err(error.into());
        }
        context.validate_private_file(&context.transport_key_path())?;
        context.validate_private_file(&context.transport_certificate_path())?;
        Ok(Self {
            private_key,
            certificate,
        })
    }

    fn load(context: &NodeContext, identity: &NodeIdentity) -> Result<Self, NodeTransportError> {
        context.validate_private_file(&context.transport_key_path())?;
        context.validate_private_file(&context.transport_certificate_path())?;
        let key = fs::read(context.transport_key_path())?;
        let private_key: [u8; 32] = key
            .try_into()
            .map_err(|_| NodeTransportError::State("transport key is malformed".to_string()))?;
        let public_key = x25519_public_from_private(&private_key)?;
        let certificate =
            TransportCertificate::from_bytes(&fs::read(context.transport_certificate_path())?)?;
        if certificate.transport_public() != &public_key {
            return Err(NodeTransportError::State(
                "transport certificate does not match transport key".to_string(),
            ));
        }
        let status = identity.public_status();
        if certificate.node_id() != status.node_id
            || hex(certificate.identity_key()) != status.public_key_hex
        {
            return Err(NodeTransportError::State(
                "transport certificate does not match node identity".to_string(),
            ));
        }
        certificate.verify_time(unix_seconds())?;
        Ok(Self {
            private_key,
            certificate,
        })
    }
}

fn random_certificate_id() -> [u8; 16] {
    let mut id = [0u8; 16];
    OsRng.fill_bytes(&mut id);
    id
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn regular_file_exists(path: &std::path::Path) -> Result<bool, NodeTransportError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(NodeTransportError::State(format!(
            "{} has an unexpected file type",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
    use tempfile::TempDir;

    fn context(temp: &TempDir) -> NodeContext {
        NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(temp.path().join("state")),
                Some(temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn load_existing_never_reprovisions_deleted_transport_state() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let provisioned = LocalTransport::provision_new(&context, &identity).unwrap();
        let certificate = provisioned.certificate().clone();
        let loaded = LocalTransport::load_existing(&context, &identity).unwrap();
        assert_eq!(loaded.certificate(), &certificate);

        fs::remove_file(context.transport_key_path()).unwrap();
        fs::remove_file(context.transport_certificate_path()).unwrap();
        assert!(matches!(
            LocalTransport::load_existing(&context, &identity),
            Err(NodeTransportError::State(message)) if message.contains("must both exist")
        ));
        assert!(!context.transport_key_path().exists());
        assert!(!context.transport_certificate_path().exists());
    }

    #[test]
    fn provision_new_refuses_to_replace_existing_transport_state() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        LocalTransport::provision_new(&context, &identity).unwrap();
        assert!(matches!(
            LocalTransport::provision_new(&context, &identity),
            Err(NodeTransportError::State(message)) if message.contains("already exists")
        ));
    }
}
