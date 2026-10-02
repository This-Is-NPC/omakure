use super::codes::CueCode;
use super::session::CuePolicy;
use super::{CAPABILITY_NOTIFICATIONS, CAPABILITY_REMOTE_RUN, MAX_LIFETIME_SECONDS};
use crate::node_registry::health::HealthAuthorization;
use crate::node_registry::{PeerRole, PeerState};
use crate::util::hex;
use std::fs::{File, OpenOptions};
use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecutionLockError {
    #[error("cannot prepare Cue execution lock: {0}")]
    Prepare(#[source] crate::node::NodeError),
    #[error("cannot open Cue execution lock: {0}")]
    Open(#[source] io::Error),
    #[error("cannot acquire Cue execution lock: {0}")]
    Acquire(#[source] io::Error),
}

/// Serialize authorization changes with the final worker check and process spawn.
pub struct ExecutionGuard {
    _file: File,
}

impl ExecutionGuard {
    pub fn acquire(
        context: &crate::node::NodeContext,
        actor: &str,
    ) -> Result<Self, ExecutionLockError> {
        use fs2::FileExt;
        use sha2::{Digest, Sha256};

        context
            .ensure_state_directory()
            .map_err(ExecutionLockError::Prepare)?;
        let digest = Sha256::digest(actor.as_bytes());
        let path = context
            .state_dir()
            .join(format!(".cue-execution-{digest:x}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(ExecutionLockError::Open)?;
        file.lock_exclusive().map_err(ExecutionLockError::Acquire)?;
        Ok(Self { _file: file })
    }
}

/// Everything the gates read, all of it local to the receiver.
///
/// Constructed by the caller from this node's own configuration and registry.
/// There is deliberately no way to build one from an inbound payload.
#[derive(Debug, Clone)]
pub struct LocalAuthority {
    /// `trust.allow_remote_cues` from this node's own config.
    pub remote_cues_enabled: bool,
    /// The sender's authorization as this node records it, if it knows the peer.
    pub authorization: Option<HealthAuthorization>,
    /// `trust.remote_cue_scripts`: what this node has declared it will run on
    /// another node's orders. Empty means nothing.
    pub declared_scripts: Vec<String>,
    /// `trust.remote_cue_batteries`: batteries whose installed scripts count as
    /// declared. Empty means none.
    pub declared_batteries: Vec<String>,
}

/// The gate decision. `Accepted` means the four gates passed; it does not mean
/// anything will run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Accepted,
    Rejected(CueCode),
}

/// Evaluate the four frozen gates, fail-closed, in order.
///
/// Order matters for what a sender can learn: gate A is checked first, so a node
/// with Cues disabled produces the same silence for every peer regardless of
/// what it knows about them.
pub fn evaluate_gates(authority: &LocalAuthority) -> GateDecision {
    // A — this node has not opted in. `allow_remote_cues` defaults to false, so
    // a node that never declared itself refuses every Cue.
    if !authority.remote_cues_enabled {
        return GateDecision::Rejected(CueCode::Disabled);
    }

    // B — the sender must be a peer this node currently trusts, in the
    // conductor role. A peer it has never heard of fails here, as does a
    // revoked or suspended one.
    let Some(authorization) = authority.authorization.as_ref() else {
        return GateDecision::Rejected(CueCode::NotActiveConductor);
    };
    if authorization.role != PeerRole::Conductor || authorization.state != PeerState::Active {
        return GateDecision::Rejected(CueCode::NotActiveConductor);
    }

    // C — and hold the capability to ask for a run.
    if !holds(authorization, CAPABILITY_REMOTE_RUN) {
        return GateDecision::Rejected(CueCode::MissingRemoteRun);
    }

    // D — and be able to receive the outcome. Accepting without this would be a
    // promise the node cannot keep: the run would happen and the Conductor
    // could never learn what came of it.
    if !holds(authorization, CAPABILITY_NOTIFICATIONS) {
        return GateDecision::Rejected(CueCode::MissingNotifications);
    }

    GateDecision::Accepted
}

pub(super) fn holds(authorization: &HealthAuthorization, capability: &str) -> bool {
    authorization
        .capabilities
        .iter()
        .any(|held| held == capability)
}

/// Gate E: the named script must be declared in `trust.remote_cue_scripts`.
///
/// Evaluated only after the four trust gates have passed, so an unauthorized
/// peer cannot use rejection codes to learn which scripts a node declares.
///
/// Deny-by-default: an empty or absent list means nothing runs remotely, no
/// matter what else is configured. This is the switch that makes "what may run"
/// a thing someone wrote down rather than a consequence of what happens to be
/// in a directory.
pub fn is_declared(name: &str, declared: &[String]) -> Result<(), CueCode> {
    if declared.iter().any(|entry| entry == name) {
        Ok(())
    } else {
        Err(CueCode::NotDeclared)
    }
}

/// Gate E, both forms: named outright, or installed by a declared battery.
///
/// A battery is a versioned set with recorded provenance, so declaring one is a
/// verifiable statement about a source rather than a wildcard. The provenance
/// is read from the local install record, never from the message.
pub fn is_declared_or_from_declared_battery(
    name: &str,
    resolved: &std::path::Path,
    policy: &CuePolicy,
    workspace: &crate::workspace::Workspace,
) -> Result<(), CueCode> {
    if is_declared(name, &policy.declared_scripts).is_ok() {
        return Ok(());
    }
    if policy.declared_batteries.is_empty() {
        return Err(CueCode::NotDeclared);
    }
    match crate::operations::battery::installing_battery(
        workspace,
        &policy.declared_batteries,
        resolved,
    ) {
        Some(_) => Ok(()),
        None => Err(CueCode::NotDeclared),
    }
}

/// Whether a Cue is inside its own validity window.
///
/// Checked at receive and again at the accept transition, so a message that
/// expired in between lands `Expired` rather than `Accepted`.
pub fn within_validity_window(not_before: i64, expires_at: i64, now: i64) -> Result<(), CueCode> {
    if expires_at <= not_before || expires_at - not_before > MAX_LIFETIME_SECONDS {
        return Err(CueCode::InvalidMessage);
    }
    if now < not_before || now >= expires_at {
        return Err(CueCode::Expired);
    }
    Ok(())
}

/// The frozen `cue_id` grammar: 32 lowercase hex chars.
pub fn is_well_formed_cue_id(cue_id: &str) -> bool {
    cue_id.len() == crate::health_plane::bounds::OPAQUE_ID_HEX_CHARS && hex::is_lower(cue_id)
}

/// The frozen script-name grammar: `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`.
///
/// This is a *shape* check, never the authorization decision. A well-formed name
/// still has to resolve inside the discoverable workspace, which is what
/// actually constrains what may run.
pub fn is_well_formed_script_name(name: &str) -> bool {
    if name.is_empty() || name.len() > crate::health_plane::bounds::MAX_SCRIPT_BYTES {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Resolve a Cue's script name against the discoverable workspace listing.
///
/// The listing **is** the allow-list. It is produced by the workspace
/// repository, which already honours `.omakureignore`, so a script the owner
/// excluded from discovery is not remotely runnable and there is no second
/// mechanism that could drift out of step with the first.
///
/// Resolution is a match against that set, never a string check on the name and
/// never a path join. A name cannot therefore address anything the owner did not
/// already publish, and traversal, absolute paths, and nested paths fail on the
/// grammar before they are ever compared.
///
/// `listing` is expected to contain absolute paths as the repository produces
/// them; only the final component is compared.
pub fn resolve_in_listing<'a>(
    name: &str,
    listing: &'a [std::path::PathBuf],
) -> Result<&'a std::path::Path, CueCode> {
    if !is_well_formed_script_name(name) {
        return Err(CueCode::InvalidMessage);
    }
    listing
        .iter()
        .find(|candidate| {
            candidate
                .file_name()
                .and_then(|component| component.to_str())
                .is_some_and(|component| component == name)
        })
        .map(std::path::PathBuf::as_path)
        .ok_or(CueCode::ScriptUnresolvable)
}

/// Reject anything that is not a regular file.
///
/// `symlink_metadata` does not follow links, so a symlink inside the workspace
/// cannot redirect a Cue to a file outside it. Directories, sockets, and FIFOs
/// fail here too.
pub fn is_regular_file(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

/// Whether a script's schema asks for any secret.
///
/// Such a script is refused at the gate rather than executed without its
/// secrets. A remote caller does not get to decide that a secret-consuming
/// script should run in a degraded form.
pub fn declares_secret_field(schema: &crate::domain::Schema) -> bool {
    schema.fields.iter().any(|field| field.is_secret())
}
