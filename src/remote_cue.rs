//! The receive half of the Remote Cue plane, frozen by
//! `.docs/remote-cue-contract.md`.
//!
//! This module decides **whether a node will accept an instruction at all**. It
//! executes nothing, and deliberately holds no path into `run_executor`,
//! `runs::enqueue`, or `runs::start_inline`. The security boundary lands and is
//! certified before any code path can cause work to run.
//!
//! Every authorization input is read from the receiver's own registry and
//! configuration. No field of an inbound message contributes to the decision to
//! accept it, so a Cue asserting its own role or capability is refused whenever
//! the local registry disagrees. The gate logic below is a pure function over
//! locally-read facts precisely so that property is visible rather than
//! asserted.

use crate::node_registry::health::HealthAuthorization;
use crate::node_registry::{PeerRole, PeerState};
use crate::ports::ScriptRepository;
use rand::rngs::OsRng;
use rand::RngCore;

/// Stable rejection codes, frozen in `.docs/remote-cue-contract.md`.
///
/// The band is `1201..` inside the existing `transport_audit.error_code` range
/// `1000..=1999`, disjoint from transport `1001..=1011`/`1020` and Health
/// `1101..=1115`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueCode {
    Disabled,
    NotActiveConductor,
    MissingRemoteRun,
    MissingNotifications,
    NotDeclared,
    ScriptDeclaresSecrets,
    ScriptUnresolvable,
    Expired,
    Duplicate,
    RateLimited,
    RunAlreadyInFlight,
    InvalidMessage,
}

impl CueCode {
    /// The stable code, typed to the width of the `transport_audit` column it
    /// is written to so no cast can silently truncate it.
    pub fn code(self) -> u16 {
        match self {
            CueCode::Disabled => 1201,
            CueCode::NotActiveConductor => 1202,
            CueCode::MissingRemoteRun => 1203,
            CueCode::MissingNotifications => 1204,
            CueCode::NotDeclared => 1212,
            CueCode::ScriptDeclaresSecrets => 1205,
            CueCode::ScriptUnresolvable => 1206,
            CueCode::Expired => 1207,
            CueCode::Duplicate => 1208,
            CueCode::RateLimited => 1209,
            CueCode::RunAlreadyInFlight => 1210,
            CueCode::InvalidMessage => 1211,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CueCode::Disabled => "cue_disabled",
            CueCode::NotActiveConductor => "cue_not_active_conductor",
            CueCode::MissingRemoteRun => "cue_missing_remote_run",
            CueCode::MissingNotifications => "cue_missing_notifications",
            CueCode::NotDeclared => "cue_script_not_declared",
            CueCode::ScriptDeclaresSecrets => "cue_script_declares_secrets",
            CueCode::ScriptUnresolvable => "cue_script_unresolvable",
            CueCode::Expired => "cue_expired",
            CueCode::Duplicate => "cue_duplicate",
            CueCode::RateLimited => "cue_rate_limited",
            CueCode::RunAlreadyInFlight => "cue_run_already_in_flight",
            CueCode::InvalidMessage => "cue_invalid_message",
        }
    }

    /// The code this refusal is *reported* as, which is not always the code it
    /// is *audited* as.
    ///
    /// `NotDeclared` is audited distinctly, because the operator of the
    /// receiving node genuinely wants to know that someone asked for a script
    /// they never declared. It is reported as `ScriptUnresolvable`, because
    /// telling an authorized Conductor the difference between "exists but is
    /// not declared" and "does not exist" lets it enumerate the workspace by
    /// elimination — the same oracle the contract already closed by collapsing
    /// missing and ignored into one code.
    pub fn reply_code(self) -> CueCode {
        match self {
            CueCode::NotDeclared => CueCode::ScriptUnresolvable,
            other => other,
        }
    }

    /// Whether a refusal with this code may be told to the sender.
    ///
    /// Follows the Health Plane precedent: trust, role, and capability failures
    /// are dropped and audited only, so an unauthorized peer learns nothing —
    /// not even that remote Cues exist on this node. Everything else is a
    /// message the sender is already authorized to have evaluated.
    pub fn is_reportable(self) -> bool {
        !matches!(
            self,
            CueCode::Disabled
                | CueCode::NotActiveConductor
                | CueCode::MissingRemoteRun
                | CueCode::MissingNotifications
        )
    }
}

/// The capability required to send a Cue at all.
pub const CAPABILITY_REMOTE_RUN: &str = "remote-run";
/// Required too, because a peer that cannot receive an outcome must not be able
/// to create work whose result is unobservable.
pub const CAPABILITY_NOTIFICATIONS: &str = "notifications";

/// The frozen maximum lifetime of a Cue, in seconds.
pub const MAX_LIFETIME_SECONDS: i64 = 300;

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
    // A — this node has not opted in. Until now `allow_remote_cues` was parsed,
    // defaulted false, and reported, but read by no enforcement path at all.
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

fn holds(authorization: &HealthAuthorization, capability: &str) -> bool {
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

/// The two kinds of the Cue plane, frozen by the contract.
pub const KIND_DISPATCH: &str = "cue_dispatch";
pub const KIND_ACK: &str = "cue_ack";

/// The `cue_dispatch` payload, after shape validation.
///
/// Parsing is total and rejects anything outside the frozen grammar, so every
/// field below is already within its bound by the time a gate reads it. None of
/// them is an authorization input: they say *what* was asked for, never whether
/// it is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CueDispatch {
    cue_id: String,
    script: String,
    not_before: i64,
    expires_at: i64,
    reason: String,
}

impl CueDispatch {
    fn parse(payload: &serde_json::Value) -> Option<Self> {
        let object = payload.as_object()?;
        if object.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
            return None;
        }
        let cue_id = object.get("cue_id")?.as_str()?.to_string();
        if cue_id.len() != crate::health_plane::bounds::OPAQUE_ID_HEX_CHARS
            || !cue_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        let script = object.get("script")?.as_str()?.to_string();
        if !is_well_formed_script_name(&script) {
            return None;
        }
        let reason = object.get("reason")?.as_str()?.to_string();
        if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
            return None;
        }
        let not_before = object.get("not_before")?.as_i64()?;
        let expires_at = object.get("expires_at")?.as_i64()?;
        if not_before < 1 || expires_at < 1 {
            return None;
        }
        Some(Self {
            cue_id,
            script,
            not_before,
            expires_at,
            reason,
        })
    }
}

/// The frozen upper bound on a Cue's human-readable reason.
pub const MAX_REASON_BYTES: usize = 128;

/// What gate E authorized: which file, and exactly which bytes.
///
/// Carried from the gate to the accept transition so the two can be compared.
/// A name is not enough -- the whole point is that the *content* authorized is
/// the content enqueued.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptBinding {
    path: std::path::PathBuf,
    content_hash: String,
}

/// SHA-256 of a script's bytes, or `None` if it cannot be read.
///
/// Unreadable is not "unchanged": a missing or unreadable file must fail the
/// comparison rather than pass it, so the caller treats `None` as a mismatch.
fn content_hash(path: &std::path::Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// UTC Unix seconds, for the second validity check at the accept transition.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// The receive-side Cue session.
///
/// Holds only what the gates read, all of it local. It is constructed beside a
/// `HealthSession` from the same session facts, and it deliberately borrows the
/// registry rather than owning a channel to anything that can run work.
pub struct CueSession<'a> {
    registry: &'a crate::node_registry::NodeRegistry,
    /// This node's own signing identity, used only to sign a `cue_ack`.
    identity: &'a crate::node_identity::NodeIdentity,
    /// The sender's identity key, as the *handshake* established it. Envelope
    /// verification is anchored to this rather than to anything the message
    /// says about itself.
    remote_identity_key: [u8; 32],
    /// The transport session this Cue must belong to. An envelope minted for a
    /// different session fails verification, so a captured Cue cannot be
    /// replayed onto a new connection.
    session_id: [u8; 32],
    /// The workspace whose declared scripts a Cue may name.
    ///
    /// `None` means decide and audit but never enqueue: a node with no
    /// workspace has nothing to run, and should say so rather than pretend.
    workspace: Option<crate::workspace::Workspace>,
    remote_node_id: String,
    remote_cues_enabled: bool,
    declared_scripts: Vec<String>,
    declared_batteries: Vec<String>,
    /// Cue ids already decided on this session.
    ///
    /// Deliberately in-session only. It covers the realistic duplicate — a
    /// retransmission on a live connection — and nothing else, which is honest
    /// about what it is.
    ///
    /// Durable at-most-once does not belong here, because in this wave
    /// "accepting" writes an audit row and nothing more: a duplicate is a
    /// cosmetic repeat in a log, not a repeated side effect. It starts
    /// mattering when acceptance causes work, and at that point the natural key
    /// already exists -- `runs.run_id` is a TEXT PRIMARY KEY derived from the
    /// cue id, so the database refuses the second insert itself.
    ///
    /// Reusing `health_replay_keys` was considered and rejected: it evicts rows
    /// older than the 180-second replay security floor, while a Cue may live
    /// 300 seconds, so a still-valid cue id could be evicted under capacity
    /// pressure. The numbers do not fit.
    seen_cue_ids: std::collections::HashSet<String>,
    /// A signed `cue_ack` the dispatcher should write back, if the refusal is
    /// one this sender is allowed to be told about.
    pending_reply: Option<Vec<u8>>,
}

/// What the dispatcher should do with an inbound Cue frame.
///
/// The decision is carried out rather than collapsed into "handled". A caller
/// that cannot tell a fresh decision from a repeat cannot assert on either, and
/// a test written against such a type passes for reasons unrelated to what it
/// claims to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueOutcome {
    /// Not Cue traffic; the dispatcher keeps its existing behaviour.
    NotCue,
    /// Decided and audited for the first time on this session.
    Decided(GateDecision),
    /// A cue id already decided on this session; answered from the first
    /// decision rather than evaluated again.
    Repeat,
}

/// Read `trust.allow_remote_cues` from this node's own configuration.
///
/// Read per session rather than cached at service start, so turning Cues off
/// takes effect on the next session instead of requiring a restart. Any failure
/// to read is `false`: a node that cannot prove it opted in has not opted in.
/// What this node has declared about remote execution.
#[derive(Debug, Clone, Default)]
pub struct CuePolicy {
    pub enabled: bool,
    pub declared_scripts: Vec<String>,
    pub declared_batteries: Vec<String>,
}

/// Read the declared remote-execution policy from this node's own config.
///
/// Read per session rather than cached at start, so a change takes effect on
/// the next session instead of requiring a restart. Any failure to read yields
/// the default, which denies everything: a node that cannot prove what it
/// declared has declared nothing.
pub fn read_policy(context: &crate::node::NodeContext) -> CuePolicy {
    let Ok(Some(mut file)) = context.open_public_file() else {
        return CuePolicy::default();
    };
    let mut contents = String::new();
    if std::io::Read::read_to_string(&mut file, &mut contents).is_err() {
        return CuePolicy::default();
    }
    crate::domain::NodeConfig::parse(&contents)
        .map(|config| CuePolicy {
            enabled: config.trust.allow_remote_cues,
            declared_scripts: config.trust.remote_cue_scripts,
            declared_batteries: config.trust.remote_cue_batteries,
        })
        .unwrap_or_default()
}

impl<'a> CueSession<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: &'a crate::node_registry::NodeRegistry,
        identity: &'a crate::node_identity::NodeIdentity,
        remote_node_id: &str,
        remote_identity_key: [u8; 32],
        session_id: [u8; 32],
        policy: CuePolicy,
        workspace: Option<crate::workspace::Workspace>,
    ) -> Self {
        Self {
            registry,
            identity,
            remote_identity_key,
            session_id,
            workspace,
            remote_node_id: remote_node_id.to_string(),
            remote_cues_enabled: policy.enabled,
            declared_scripts: policy.declared_scripts,
            declared_batteries: policy.declared_batteries,
            seen_cue_ids: std::collections::HashSet::new(),
            pending_reply: None,
        }
    }

    /// Decide one inbound envelope, end to end.
    ///
    /// Returns `NotCue` for anything outside the `cue_` namespace so the
    /// dispatcher's existing fall-through is preserved exactly.
    ///
    /// Everything a decision reads is either local -- the registry, this node's
    /// own config, its own workspace listing -- or bound to the transport
    /// session by `verify_envelope`. The message supplies the *subject* of the
    /// decision, which script and which cue id, and never an input to it.
    pub fn handle_envelope(&mut self, encoded: &[u8], now: i64) -> CueOutcome {
        let Some(kind) = crate::direct_transport::envelope_kind_hint(encoded) else {
            return CueOutcome::NotCue;
        };
        if !kind.starts_with(crate::direct_transport::CUE_KIND_PREFIX) {
            return CueOutcome::NotCue;
        }
        // A `cue_ack` is the Conductor's half of the protocol. A Performer
        // receiving one has been sent a message for the other direction, which
        // is malformed traffic rather than an instruction.
        if kind != KIND_DISPATCH {
            return self.refuse(None, CueCode::InvalidMessage, now);
        }

        // Verification is anchored to the handshake identity and this session
        // id, so a Cue captured from one connection cannot be replayed onto
        // another, and a signature from anyone but the peer we handshook with
        // is not a Cue at all.
        let verified = crate::direct_transport::envelope_nonce(encoded).and_then(|nonce| {
            crate::direct_transport::verify_envelope(
                encoded,
                &self.remote_node_id,
                &self.remote_identity_key,
                kind,
                &self.session_id,
                &nonce,
            )
        });
        if verified.is_err() {
            return self.refuse(None, CueCode::InvalidMessage, now);
        }

        let Ok(view) = crate::direct_transport::envelope_view(encoded) else {
            return self.refuse(None, CueCode::InvalidMessage, now);
        };
        let Some(dispatch) = CueDispatch::parse(&view.payload) else {
            return self.refuse(None, CueCode::InvalidMessage, now);
        };

        if !self.seen_cue_ids.insert(dispatch.cue_id.clone()) {
            self.audit("cue_rejected", "rejected", Some(CueCode::Duplicate));
            return CueOutcome::Repeat;
        }

        if let Err(code) = within_validity_window(dispatch.not_before, dispatch.expires_at, now) {
            return self.refuse(Some(&dispatch), code, now);
        }
        if let GateDecision::Rejected(code) = evaluate_gates(&self.authority()) {
            return self.refuse(Some(&dispatch), code, now);
        }
        let binding = match self.authorize_script(&dispatch.script) {
            Ok(binding) => binding,
            Err(code) => return self.refuse(Some(&dispatch), code, now),
        };

        // Checked again at the accept transition. The gates above read the
        // registry and walk the workspace; a Cue that expired while they ran
        // must land `Expired`, not become a run.
        let at_accept = unix_now();
        if let Err(code) =
            within_validity_window(dispatch.not_before, dispatch.expires_at, at_accept)
        {
            return self.refuse(Some(&dispatch), code, at_accept);
        }

        // The gates walked the filesystem and read a schema. Re-check the file
        // that was authorized is still the file that will be enqueued, so a
        // swap during that walk cannot ride an authorization granted for
        // different content.
        if content_hash(&binding.path).as_deref() != Some(binding.content_hash.as_str()) {
            return self.refuse(Some(&dispatch), CueCode::ScriptUnresolvable, at_accept);
        }
        if let GateDecision::Rejected(code) = evaluate_gates(&self.authority()) {
            return self.refuse(Some(&dispatch), code, at_accept);
        }

        match self.enqueue_accepted(&dispatch.cue_id, &dispatch.script, &dispatch.reason) {
            Ok(_) => {
                self.audit("cue_accepted", "accepted", None);
                self.queue_reply(&dispatch.cue_id, None, at_accept);
                CueOutcome::Decided(GateDecision::Accepted)
            }
            // The run id is derived from the cue id and is the table's primary
            // key, so a refused insert means this Cue already became a run.
            // Failing here is the at-most-once guarantee working.
            Err(code) => self.refuse(Some(&dispatch), code, at_accept),
        }
    }

    /// Gate E, in the order that leaks least.
    ///
    /// Resolution runs before the declaration check so that "declared but
    /// absent" and "present but undeclared" both end at the same reported
    /// code; the audited codes still differ, so the owner can tell them apart
    /// locally while the sender cannot.
    fn authorize_script(&self, script: &str) -> Result<ScriptBinding, CueCode> {
        let workspace = self.workspace.as_ref().ok_or(CueCode::ScriptUnresolvable)?;
        let repo = crate::adapters::workspace_repository::FsWorkspaceRepository::new(
            workspace.scripts_root().to_path_buf(),
        );
        let listing = repo
            .list_scripts_recursive()
            .map_err(|_| CueCode::ScriptUnresolvable)?;
        let resolved = resolve_in_listing(script, &listing)?;
        if !is_regular_file(resolved) {
            return Err(CueCode::ScriptUnresolvable);
        }
        is_declared_or_from_declared_battery(
            script,
            resolved,
            &CuePolicy {
                enabled: self.remote_cues_enabled,
                declared_scripts: self.declared_scripts.clone(),
                declared_batteries: self.declared_batteries.clone(),
            },
            workspace,
        )?;
        let schema = repo
            .read_schema(resolved)
            .map_err(|_| CueCode::ScriptUnresolvable)?;
        if declares_secret_field(&schema) {
            return Err(CueCode::ScriptDeclaresSecrets);
        }
        Ok(ScriptBinding {
            path: resolved.to_path_buf(),
            content_hash: content_hash(resolved).ok_or(CueCode::ScriptUnresolvable)?,
        })
    }

    fn authority(&self) -> LocalAuthority {
        LocalAuthority {
            remote_cues_enabled: self.remote_cues_enabled,
            declared_scripts: self.declared_scripts.clone(),
            declared_batteries: self.declared_batteries.clone(),
            authorization: self
                .registry
                .health_authorization(&self.remote_node_id)
                .ok()
                .flatten(),
        }
    }

    fn audit(&self, event: &str, outcome: &str, code: Option<CueCode>) {
        let _ = self.registry.record_transport_audit(
            event,
            &self.remote_node_id,
            Some(&self.session_id),
            None,
            0,
            outcome,
            code.map(CueCode::code),
        );
    }

    /// Audit the true code, report the narrowed one, and only to a sender
    /// already authorized to have been evaluated.
    fn refuse(&mut self, dispatch: Option<&CueDispatch>, code: CueCode, now: i64) -> CueOutcome {
        self.audit("cue_rejected", "rejected", Some(code));
        if code.is_reportable() {
            if let Some(dispatch) = dispatch {
                let reported = code.reply_code();
                self.queue_reply(&dispatch.cue_id, Some(reported), now);
            }
        }
        CueOutcome::Decided(GateDecision::Rejected(code))
    }

    fn queue_reply(&mut self, cue_id: &str, code: Option<CueCode>, now: i64) {
        let Ok(created_at) = u64::try_from(now) else {
            return;
        };
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        // The shape is the frozen reference vector in
        // `tests/remote_cue_contract.rs`: flat, with `error` present only on a
        // refusal, so "accepted" is never expressed as a code of zero.
        let mut payload = serde_json::json!({
            "version": 1,
            "cue_id": cue_id,
            "accepted": code.is_none(),
        });
        if let Some(code) = code {
            payload["error"] = serde_json::json!({ "code": code.code() });
        }
        self.pending_reply = crate::direct_transport::sign_cue_envelope(
            self.identity,
            KIND_ACK,
            &self.session_id,
            nonce,
            payload,
            created_at,
        )
        .ok()
        .map(|envelope| envelope.encoded());
    }

    /// The signed `cue_ack` this session owes the sender, if any.
    pub fn take_reply(&mut self) -> Option<Vec<u8>> {
        self.pending_reply.take()
    }

    /// Decide one Cue, optionally identified so a retransmission on this
    /// session is answered from the first decision rather than re-evaluated.
    ///
    /// Re-evaluating would not be unsafe -- the gates are pure over local state
    /// and would reach the same answer -- but it would write a second audit row
    /// for one instruction, which makes the trail harder to read for no gain.
    pub fn decide(&mut self, cue_id: Option<&str>) -> CueOutcome {
        if let Some(cue_id) = cue_id {
            if !self.seen_cue_ids.insert(cue_id.to_string()) {
                let _ = self.registry.record_transport_audit(
                    "cue_rejected",
                    &self.remote_node_id,
                    None,
                    None,
                    0,
                    "rejected",
                    Some(CueCode::Duplicate.code()),
                );
                return CueOutcome::Repeat;
            }
        }

        let authority = LocalAuthority {
            remote_cues_enabled: self.remote_cues_enabled,
            declared_scripts: self.declared_scripts.clone(),
            declared_batteries: self.declared_batteries.clone(),
            authorization: self
                .registry
                .health_authorization(&self.remote_node_id)
                .ok()
                .flatten(),
        };

        let decision = evaluate_gates(&authority);
        let (outcome, code) = match decision {
            GateDecision::Accepted => ("accepted", None),
            GateDecision::Rejected(code) => ("rejected", Some(code)),
        };

        // Audit every decision, including acceptance. A remote instruction that
        // left no trace would defeat the point of the roadmap item it belongs
        // to, whose scope is distributed audit outcomes.
        let _ = self.registry.record_transport_audit(
            if code.is_some() {
                "cue_rejected"
            } else {
                "cue_accepted"
            },
            &self.remote_node_id,
            None,
            None,
            0,
            outcome,
            code.map(CueCode::code),
        );

        CueOutcome::Decided(decision)
    }

    /// Turn an accepted decision into one run.
    ///
    /// Separated from `decide` so the security boundary and the act of running
    /// stay reviewable apart: everything above answers "may this happen", and
    /// only this answers "make it happen".
    ///
    /// The run id is supplied by the caller and derived from the cue id, so the
    /// primary key refuses a second insert. Enqueue therefore *failing* is the
    /// success path for a duplicate, not an error to paper over.
    pub fn enqueue_accepted(
        &self,
        cue_id: &str,
        script: &str,
        reason: &str,
    ) -> Result<String, CueCode> {
        let workspace = self.workspace.as_ref().ok_or(CueCode::ScriptUnresolvable)?;
        let run_id = derive_run_id(cue_id);
        crate::operations::core::enqueue_cue_run(
            workspace,
            crate::operations::core::EnqueueRunRequest {
                script: script.to_string(),
                args: Vec::new(),
                env: None,
                secret_fields: Vec::new(),
                run_id: Some(run_id.clone()),
                actor: self.remote_node_id.clone(),
                reason: Some(reason.to_string()),
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
            },
        )
        .map(|_| run_id)
        .map_err(|_| CueCode::Duplicate)
    }
}

/// The local run id for a Cue, under its own domain separator.
///
/// Deterministic so the Conductor can compute the opaque run id it will see on
/// the `run-completed` Signal without any message carrying a correlation field,
/// and so the database primary key is the durable at-most-once guard.
///
/// The domain separator is what stops a cue id being replayable as a preimage
/// in any other construction that hashes ids.
pub fn derive_run_id(cue_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"omakure/cue-run-id/v1\0");
    hasher.update(cue_id.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authorization(
        role: PeerRole,
        state: PeerState,
        capabilities: &[&str],
    ) -> HealthAuthorization {
        HealthAuthorization {
            node_id: "omk1_test".to_string(),
            state,
            role,
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn passing() -> LocalAuthority {
        LocalAuthority {
            remote_cues_enabled: true,
            declared_scripts: vec!["deploy.sh".to_string()],
            declared_batteries: Vec::new(),
            authorization: Some(authorization(
                PeerRole::Conductor,
                PeerState::Active,
                &[CAPABILITY_REMOTE_RUN, CAPABILITY_NOTIFICATIONS],
            )),
        }
    }

    #[test]
    fn all_four_gates_passing_accepts() {
        assert_eq!(evaluate_gates(&passing()), GateDecision::Accepted);
    }

    /// Each gate flipped alone, with the other three passing.
    ///
    /// This is the shape of the certification: a gate that only refuses when
    /// several things are wrong at once is not a gate.
    #[test]
    fn gate_a_alone_refuses_when_the_node_has_not_opted_in() {
        let mut authority = passing();
        authority.remote_cues_enabled = false;
        assert_eq!(
            evaluate_gates(&authority),
            GateDecision::Rejected(CueCode::Disabled)
        );
    }

    #[test]
    fn gate_b_alone_refuses_an_unknown_peer() {
        let mut authority = passing();
        authority.authorization = None;
        assert_eq!(
            evaluate_gates(&authority),
            GateDecision::Rejected(CueCode::NotActiveConductor)
        );
    }

    #[test]
    fn gate_b_alone_refuses_a_performer() {
        let mut authority = passing();
        authority.authorization = Some(authorization(
            PeerRole::Performer,
            PeerState::Active,
            &[CAPABILITY_REMOTE_RUN, CAPABILITY_NOTIFICATIONS],
        ));
        assert_eq!(
            evaluate_gates(&authority),
            GateDecision::Rejected(CueCode::NotActiveConductor)
        );
    }

    #[test]
    fn gate_b_alone_refuses_a_revoked_conductor() {
        let mut authority = passing();
        authority.authorization = Some(authorization(
            PeerRole::Conductor,
            PeerState::Revoked,
            &[CAPABILITY_REMOTE_RUN, CAPABILITY_NOTIFICATIONS],
        ));
        assert_eq!(
            evaluate_gates(&authority),
            GateDecision::Rejected(CueCode::NotActiveConductor)
        );
    }

    #[test]
    fn gate_c_alone_refuses_a_conductor_without_remote_run() {
        let mut authority = passing();
        authority.authorization = Some(authorization(
            PeerRole::Conductor,
            PeerState::Active,
            &[CAPABILITY_NOTIFICATIONS],
        ));
        assert_eq!(
            evaluate_gates(&authority),
            GateDecision::Rejected(CueCode::MissingRemoteRun)
        );
    }

    #[test]
    fn gate_d_alone_refuses_a_conductor_that_could_not_receive_the_outcome() {
        let mut authority = passing();
        authority.authorization = Some(authorization(
            PeerRole::Conductor,
            PeerState::Active,
            &[CAPABILITY_REMOTE_RUN],
        ));
        assert_eq!(
            evaluate_gates(&authority),
            GateDecision::Rejected(CueCode::MissingNotifications)
        );
    }

    /// Gate A is evaluated before anything peer-specific, so a node that has
    /// not opted in cannot be probed for what it knows about a peer.
    #[test]
    fn a_disabled_node_refuses_identically_for_every_peer() {
        let mut unknown = passing();
        unknown.remote_cues_enabled = false;
        unknown.authorization = None;

        let mut known = passing();
        known.remote_cues_enabled = false;

        assert_eq!(evaluate_gates(&unknown), evaluate_gates(&known));
    }

    #[test]
    fn trust_role_and_capability_refusals_are_never_reported_to_the_sender() {
        for code in [
            CueCode::Disabled,
            CueCode::NotActiveConductor,
            CueCode::MissingRemoteRun,
            CueCode::MissingNotifications,
        ] {
            assert!(
                !code.is_reportable(),
                "{} must be audited silently",
                code.name()
            );
        }
        for code in [
            CueCode::NotDeclared,
            CueCode::ScriptUnresolvable,
            CueCode::Expired,
            CueCode::Duplicate,
            CueCode::RateLimited,
            CueCode::RunAlreadyInFlight,
            CueCode::InvalidMessage,
            CueCode::ScriptDeclaresSecrets,
        ] {
            assert!(code.is_reportable(), "{} may be answered", code.name());
        }
    }

    #[test]
    fn every_code_is_unique_and_inside_the_frozen_band() {
        let codes = [
            CueCode::Disabled,
            CueCode::NotActiveConductor,
            CueCode::MissingRemoteRun,
            CueCode::MissingNotifications,
            CueCode::NotDeclared,
            CueCode::ScriptDeclaresSecrets,
            CueCode::ScriptUnresolvable,
            CueCode::Expired,
            CueCode::Duplicate,
            CueCode::RateLimited,
            CueCode::RunAlreadyInFlight,
            CueCode::InvalidMessage,
        ];
        let mut seen = std::collections::HashSet::new();
        for code in codes {
            assert!(seen.insert(code.code()), "duplicate code {}", code.code());
            assert!((1201..=1299).contains(&code.code()));
        }
        assert_eq!(seen.len(), 12);
    }

    /// Nothing runs remotely unless someone wrote it down.
    #[test]
    fn an_undeclared_script_is_refused_even_with_every_trust_gate_passing() {
        assert_eq!(
            is_declared("deploy.sh", &["restart.lua".to_string()]),
            Err(CueCode::NotDeclared)
        );
    }

    /// The switch that matters most: enabling remote Cues grants nothing on its
    /// own. Two independent deliberate acts are required.
    #[test]
    fn an_empty_declaration_denies_everything() {
        assert_eq!(is_declared("deploy.sh", &[]), Err(CueCode::NotDeclared));
        assert_eq!(is_declared("", &[]), Err(CueCode::NotDeclared));
    }

    #[test]
    fn a_declared_script_passes_the_fifth_gate() {
        let declared = vec!["deploy.sh".to_string(), "restart.lua".to_string()];
        assert_eq!(is_declared("deploy.sh", &declared), Ok(()));
        assert_eq!(is_declared("restart.lua", &declared), Ok(()));
    }

    /// Declaration is an exact match, so a near-miss cannot slip through.
    #[test]
    fn declaration_is_not_a_prefix_or_suffix_match() {
        let declared = vec!["deploy.sh".to_string()];
        for near in [
            "deploy",
            "deploy.sh.bak",
            "Deploy.sh",
            "xdeploy.sh",
            "deploy.sh ",
        ] {
            assert_eq!(
                is_declared(near, &declared),
                Err(CueCode::NotDeclared),
                "{near:?} must not match a declaration of deploy.sh"
            );
        }
    }

    /// A node that cannot read its own config has declared nothing.
    #[test]
    fn the_default_policy_denies_everything() {
        let policy = CuePolicy::default();
        assert!(!policy.enabled);
        assert!(policy.declared_scripts.is_empty());
        assert_eq!(
            is_declared("deploy.sh", &policy.declared_scripts),
            Err(CueCode::NotDeclared)
        );
    }

    /// Audited distinctly, reported indistinguishably.
    fn workspace_with_battery_script(
        battery: &str,
        script_name: &str,
    ) -> (
        tempfile::TempDir,
        crate::workspace::Workspace,
        std::path::PathBuf,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let workspace = crate::workspace::Workspace::new(dir.path().to_path_buf());
        let installed = dir.path().join(script_name);
        std::fs::write(&installed, "#!/usr/bin/env bash\n").unwrap();

        let record_dir = workspace
            .omakure_dir()
            .join("batteries")
            .join("installed")
            .join(battery);
        std::fs::create_dir_all(&record_dir).unwrap();
        std::fs::write(
            record_dir.join("record.json"),
            serde_json::json!({
                "battery_name": battery,
                "script_id": format!("{battery}.{script_name}"),
                "git_url": "https://example.invalid/b.git",
                "requested_ref": "main",
                "resolved_commit": "0".repeat(40),
                "source_path": script_name,
                "installed_path": installed,
            })
            .to_string(),
        )
        .unwrap();
        (dir, workspace, installed)
    }

    fn policy(scripts: &[&str], batteries: &[&str]) -> CuePolicy {
        CuePolicy {
            enabled: true,
            declared_scripts: scripts.iter().map(|s| s.to_string()).collect(),
            declared_batteries: batteries.iter().map(|b| b.to_string()).collect(),
        }
    }

    /// Declaring a battery declares its installed scripts.
    #[test]
    fn a_script_from_a_declared_battery_passes_without_being_named() {
        let (_dir, workspace, installed) = workspace_with_battery_script("azure", "rg-list.sh");
        assert_eq!(
            is_declared_or_from_declared_battery(
                "rg-list.sh",
                &installed,
                &policy(&[], &["azure"]),
                &workspace
            ),
            Ok(())
        );
    }

    /// And declaring one battery does not declare another.
    #[test]
    fn a_script_from_an_undeclared_battery_is_refused() {
        let (_dir, workspace, installed) = workspace_with_battery_script("azure", "rg-list.sh");
        assert_eq!(
            is_declared_or_from_declared_battery(
                "rg-list.sh",
                &installed,
                &policy(&[], &["aws"]),
                &workspace
            ),
            Err(CueCode::NotDeclared)
        );
    }

    /// A hand-written script that no battery installed stays undeclared, even
    /// when it sits beside battery scripts in the same workspace.
    #[test]
    fn a_script_with_no_provenance_is_refused_despite_a_declared_battery() {
        let (dir, workspace, _installed) = workspace_with_battery_script("azure", "rg-list.sh");
        let local = dir.path().join("local.sh");
        std::fs::write(&local, "#!/usr/bin/env bash\n").unwrap();
        assert_eq!(
            is_declared_or_from_declared_battery(
                "local.sh",
                &local,
                &policy(&[], &["azure"]),
                &workspace
            ),
            Err(CueCode::NotDeclared)
        );
    }

    /// Naming a script still works, with or without batteries in play.
    #[test]
    fn an_explicitly_named_script_needs_no_battery() {
        let (dir, workspace, _installed) = workspace_with_battery_script("azure", "rg-list.sh");
        let local = dir.path().join("deploy.sh");
        std::fs::write(&local, "#!/usr/bin/env bash\n").unwrap();
        assert_eq!(
            is_declared_or_from_declared_battery(
                "deploy.sh",
                &local,
                &policy(&["deploy.sh"], &[]),
                &workspace
            ),
            Ok(())
        );
    }

    /// Declaring nothing still denies everything.
    #[test]
    fn an_empty_policy_denies_even_battery_scripts() {
        let (_dir, workspace, installed) = workspace_with_battery_script("azure", "rg-list.sh");
        assert_eq!(
            is_declared_or_from_declared_battery(
                "rg-list.sh",
                &installed,
                &policy(&[], &[]),
                &workspace
            ),
            Err(CueCode::NotDeclared)
        );
    }

    /// Deterministic, so the Conductor computes the same id the Performer will
    /// use, without any message carrying a correlation field.
    #[test]
    fn the_run_id_is_a_deterministic_function_of_the_cue_id() {
        let a = derive_run_id("0123456789abcdef0123456789abcdef");
        assert_eq!(a, derive_run_id("0123456789abcdef0123456789abcdef"));
        assert_ne!(a, derive_run_id("fedcba9876543210fedcba9876543210"));
        assert_eq!(a.len(), 64, "a full SHA-256 in hex");
    }

    /// The domain separator is load-bearing: without it a cue id would hash the
    /// same here as in any other construction that hashes ids.
    #[test]
    fn the_run_id_is_domain_separated() {
        use sha2::{Digest, Sha256};
        let undomained: String = Sha256::digest(b"0123456789abcdef0123456789abcdef")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_ne!(
            derive_run_id("0123456789abcdef0123456789abcdef"),
            undomained
        );
    }

    #[test]
    fn not_declared_is_reported_as_unresolvable() {
        assert_eq!(
            CueCode::NotDeclared.reply_code(),
            CueCode::ScriptUnresolvable,
            "the difference would let an authorized peer enumerate the workspace"
        );
        assert_ne!(
            CueCode::NotDeclared.code(),
            CueCode::ScriptUnresolvable.code(),
            "but the receiving operator must still see which one it was"
        );
    }

    /// Every other code reports as itself; only the enumeration case collapses.
    #[test]
    fn no_other_code_is_disguised() {
        for code in [
            CueCode::Disabled,
            CueCode::NotActiveConductor,
            CueCode::MissingRemoteRun,
            CueCode::MissingNotifications,
            CueCode::ScriptDeclaresSecrets,
            CueCode::ScriptUnresolvable,
            CueCode::Expired,
            CueCode::Duplicate,
            CueCode::RateLimited,
            CueCode::RunAlreadyInFlight,
            CueCode::InvalidMessage,
        ] {
            assert_eq!(
                code.reply_code(),
                code,
                "{} must report as itself",
                code.name()
            );
        }
    }

    #[test]
    fn a_cue_outside_its_window_is_expired_rather_than_accepted() {
        assert_eq!(within_validity_window(100, 400, 99), Err(CueCode::Expired));
        assert_eq!(within_validity_window(100, 400, 400), Err(CueCode::Expired));
        assert_eq!(within_validity_window(100, 400, 100), Ok(()));
        assert_eq!(within_validity_window(100, 400, 399), Ok(()));
    }

    #[test]
    fn a_window_wider_than_the_frozen_lifetime_is_malformed() {
        assert_eq!(
            within_validity_window(100, 100 + MAX_LIFETIME_SECONDS + 1, 150),
            Err(CueCode::InvalidMessage)
        );
        assert_eq!(
            within_validity_window(100, 100, 100),
            Err(CueCode::InvalidMessage)
        );
        assert_eq!(
            within_validity_window(400, 100, 150),
            Err(CueCode::InvalidMessage)
        );
    }

    fn listing(names: &[&str]) -> Vec<std::path::PathBuf> {
        names
            .iter()
            .map(|name| std::path::PathBuf::from("/ws").join(name))
            .collect()
    }

    #[test]
    fn a_listed_script_resolves() {
        let scripts = listing(&["deploy.sh", "backup.lua"]);
        let resolved = resolve_in_listing("deploy.sh", &scripts).expect("should resolve");
        assert_eq!(resolved, std::path::Path::new("/ws/deploy.sh"));
    }

    /// The listing is the allow-list, so anything absent is simply unrunnable.
    #[test]
    fn a_script_absent_from_the_listing_is_unresolvable() {
        let scripts = listing(&["deploy.sh"]);
        assert_eq!(
            resolve_in_listing("secret.sh", &scripts),
            Err(CueCode::ScriptUnresolvable)
        );
    }

    /// An `.omakureignore`d script never appears in the listing, so exclusion
    /// from discovery is exclusion from remote execution with no second
    /// mechanism to keep in step.
    #[test]
    fn an_ignored_script_is_unresolvable_because_it_is_not_listed() {
        let scripts = listing(&["public.sh"]);
        assert_eq!(
            resolve_in_listing("private.sh", &scripts),
            Err(CueCode::ScriptUnresolvable)
        );
    }

    /// Traversal and absolute paths die on the grammar, before any comparison.
    #[test]
    fn traversal_and_absolute_names_never_reach_resolution() {
        let scripts = listing(&["deploy.sh"]);
        for hostile in [
            "../deploy.sh",
            "../../etc/passwd",
            "/etc/passwd",
            "sub/deploy.sh",
            "./deploy.sh",
        ] {
            assert_eq!(
                resolve_in_listing(hostile, &scripts),
                Err(CueCode::InvalidMessage),
                "{hostile:?} must fail the grammar"
            );
        }
    }

    /// A name matching a listed *directory* component must not resolve.
    #[test]
    fn only_the_final_component_is_compared() {
        let scripts = vec![std::path::PathBuf::from("/ws/tools/deploy.sh")];
        assert_eq!(
            resolve_in_listing("tools", &scripts),
            Err(CueCode::ScriptUnresolvable)
        );
        assert!(resolve_in_listing("deploy.sh", &scripts).is_ok());
    }

    #[test]
    fn a_symlink_is_not_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.sh");
        std::fs::write(&target, "#!/usr/bin/env bash\n").unwrap();
        let link = dir.path().join("link.sh");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(is_regular_file(&target));
        #[cfg(unix)]
        assert!(
            !is_regular_file(&link),
            "a symlink must not resolve; it could redirect outside the workspace"
        );
        assert!(!is_regular_file(dir.path()), "a directory is not a script");
        assert!(!is_regular_file(&dir.path().join("absent.sh")));
    }

    #[test]
    fn a_secret_declaring_schema_is_detected() {
        let with_secret: crate::domain::Schema = serde_json::from_value(serde_json::json!({
            "Name": "deploy",
            "Fields": [{ "Name": "token", "Type": "secret", "Required": true }]
        }))
        .unwrap();
        assert!(declares_secret_field(&with_secret));

        let without: crate::domain::Schema = serde_json::from_value(serde_json::json!({
            "Name": "deploy",
            "Fields": [{ "Name": "target", "Type": "string", "Required": false }]
        }))
        .unwrap();
        assert!(!declares_secret_field(&without));
    }

    #[test]
    fn the_script_name_grammar_is_the_frozen_one() {
        for good in ["deploy.sh", "a", "job_1.lua", "x-y.z", &"a".repeat(64)] {
            assert!(is_well_formed_script_name(good), "{good} should be valid");
        }
        for bad in [
            "",
            ".hidden",
            "-leading",
            "_leading",
            "has space",
            "sub/dir.sh",
            "../escape.sh",
            "trailing\n",
            &"a".repeat(65),
        ] {
            assert!(
                !is_well_formed_script_name(bad),
                "{bad:?} should be invalid"
            );
        }
    }

    /// A file swapped after gate E must not be enqueued under that decision.
    ///
    /// Written against `content_hash` directly because the swap window is
    /// inside one function call; a test that tried to race it would be a test
    /// of the scheduler, not of the guard.
    #[test]
    fn a_swapped_script_no_longer_matches_what_the_gate_authorized() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("deploy.sh");
        std::fs::write(&script, "echo original\n").expect("write");
        let authorized = content_hash(&script).expect("hash the authorized bytes");

        std::fs::write(&script, "echo swapped\n").expect("swap");
        assert_ne!(
            content_hash(&script).as_deref(),
            Some(authorized.as_str()),
            "the accept transition must be able to see the swap"
        );

        // And a file that vanished must fail the comparison, not pass it.
        std::fs::remove_file(&script).expect("remove");
        assert_eq!(
            content_hash(&script),
            None,
            "unreadable must never compare equal to authorized"
        );
    }
}
