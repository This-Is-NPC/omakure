use super::codes::CueCode;
use super::dispatch::{CueDecisionRecord, CueDispatch, ScriptBinding, content_hash};
use super::enqueue::CueEnqueueError;
use super::gates::{
    GateDecision, LocalAuthority, declares_secret_field, evaluate_gates,
    is_declared_or_from_declared_battery, is_regular_file, resolve_in_listing,
    within_validity_window,
};
use super::{
    CUE_RETENTION_SECONDS, KIND_ACK, KIND_DISPATCH, MAX_CANONICAL_CUE_DISPATCH,
    MAX_RETAINED_CUE_RECORDS,
};
use crate::util::entropy;
use std::collections::VecDeque;

/// The receive-side Cue session.
///
/// Holds only what the gates read, all of it local. It is constructed beside a
/// `HealthSession` from the same session facts, and it deliberately borrows the
/// registry rather than owning a channel to anything that can run work.
pub struct CueSession<'a> {
    pub(super) registry: &'a crate::node_registry::NodeRegistry,
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
    pub(super) workspace: Option<crate::workspace::Workspace>,
    pub(super) remote_node_id: String,
    policy: CuePolicy,
    /// Bounded live-session decisions. Durable at-most-once remains the run
    /// primary key; this cache exists to replay a reportable ACK without
    /// re-evaluating gates or leaking a refusal after a duplicate.
    pub(super) cue_records: VecDeque<CueDecisionRecord>,
    /// A signed `cue_ack` the dispatcher should write back, if the refusal is
    /// one this sender is allowed to be told about.
    pub(super) pending_reply: Option<Vec<u8>>,
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
    /// Enqueue failed after the authorization gates passed. This is deliberately
    /// not a Cue rejection: the failure is local to the receiver and must not be
    /// misreported as a duplicate or as a sender error.
    EnqueueFailed(CueEnqueueError),
}

/// What this node has declared about remote execution.
#[derive(Debug, Clone, Default)]
pub struct CuePolicy {
    pub enabled: bool,
    pub declared_scripts: Vec<String>,
    pub declared_batteries: Vec<String>,
}

/// Peer facts established by the authenticated transport session.
pub struct CuePeer<'a> {
    pub node_id: &'a str,
    pub identity_key: [u8; 32],
    pub session_id: [u8; 32],
}

/// Read the declared remote-execution policy from this node's own config.
///
/// Read per session rather than cached at start, so a change takes effect on
/// the next session instead of requiring a restart. Any failure to read yields
/// the default, which denies everything: a node that cannot prove what it
/// declared has declared nothing.
pub fn read_policy(context: &crate::node::NodeContext) -> CuePolicy {
    let config = match crate::node::read_policy_config(context) {
        crate::node::PolicyConfig::Declared(config) => *config,
        // Nothing declared is the shipped state and needs no comment.
        crate::node::PolicyConfig::NothingDeclared => return CuePolicy::default(),
        // A config this node cannot read denies exactly the same way, so the
        // reason has to be said out loud or a mode bit turns Cues off with
        // nothing to distinguish it from never having opted in.
        crate::node::PolicyConfig::Unreadable(reason) => {
            crate::node::warn_policy_unreadable("remote cues", &reason);
            return CuePolicy::default();
        }
    };
    CuePolicy {
        enabled: config.trust.allow_remote_cues,
        declared_scripts: config.trust.remote_cue_scripts,
        declared_batteries: config.trust.remote_cue_batteries,
    }
}

impl<'a> CueSession<'a> {
    pub fn new(
        registry: &'a crate::node_registry::NodeRegistry,
        identity: &'a crate::node_identity::NodeIdentity,
        peer: CuePeer<'_>,
        policy: CuePolicy,
        workspace: Option<crate::workspace::Workspace>,
    ) -> Self {
        Self {
            registry,
            identity,
            remote_identity_key: peer.identity_key,
            session_id: peer.session_id,
            workspace,
            remote_node_id: peer.node_id.to_string(),
            policy,
            cue_records: VecDeque::new(),
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
        self.pending_reply = None;
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
        let canonical_len = encoded.len().saturating_sub(64);
        if canonical_len > MAX_CANONICAL_CUE_DISPATCH {
            return self.refuse(None, CueCode::InvalidMessage, now);
        }
        let Some(dispatch) = CueDispatch::parse(&view.payload) else {
            return self.refuse(None, CueCode::InvalidMessage, now);
        };

        self.prune_cue_state(now);
        if let Some(record) = self
            .cue_records
            .iter()
            .find(|record| record.cue_id == dispatch.cue_id)
        {
            self.pending_reply = record.reply.clone();
            self.audit(
                "cue_rejected",
                "rejected",
                Some(CueCode::Duplicate),
                Some(&dispatch),
            );
            return CueOutcome::Repeat;
        }
        match self.registry.consume_cue_rate(&self.remote_node_id, now) {
            Ok(true) => {}
            Ok(false) => return self.refuse(Some(&dispatch), CueCode::RateLimited, now),
            Err(_) => {
                self.audit("cue_rate_check_failed", "internal", None, Some(&dispatch));
                self.remember_cue(&dispatch.cue_id, now);
                return CueOutcome::EnqueueFailed(CueEnqueueError::Failed(
                    crate::operations::OperationErrorCode::IoFailed,
                ));
            }
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
        let at_accept = crate::util::time::unix_seconds() as i64;
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

        match self.enqueue_accepted(
            &dispatch.cue_id,
            &dispatch.script,
            &dispatch.reason,
            &binding.content_hash,
        ) {
            Ok(_) => {
                self.audit("cue_accepted", "accepted", None, Some(&dispatch));
                self.queue_reply(&dispatch.cue_id, None, at_accept);
                self.remember_cue(&dispatch.cue_id, at_accept);
                CueOutcome::Decided(GateDecision::Accepted)
            }
            Err(CueEnqueueError::Duplicate) => {
                // The run id is derived from the cue id and is the table's
                // primary key, so this specific refused insert means this Cue
                // already became a run. Failing here is the at-most-once
                // guarantee working.
                self.refuse(Some(&dispatch), CueCode::Duplicate, at_accept)
            }
            Err(error) => {
                // There is no Cue wire code for a local storage/operation
                // failure. Do not fabricate one: no acknowledgement means the
                // sender cannot mistake a local fault for acceptance or a
                // duplicate, while the stable operation code remains visible
                // to the caller and in this session outcome.
                self.audit(
                    "cue_enqueue_failed",
                    error.stable_name(),
                    None,
                    Some(&dispatch),
                );
                self.remember_cue(&dispatch.cue_id, at_accept);
                CueOutcome::EnqueueFailed(error)
            }
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
        is_declared_or_from_declared_battery(script, resolved, &self.policy, workspace)?;
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
            remote_cues_enabled: self.policy.enabled,
            declared_scripts: self.policy.declared_scripts.clone(),
            declared_batteries: self.policy.declared_batteries.clone(),
            authorization: self
                .registry
                .health_authorization(&self.remote_node_id)
                .ok()
                .flatten(),
        }
    }

    fn audit(
        &self,
        event: &str,
        outcome: &str,
        code: Option<CueCode>,
        dispatch: Option<&CueDispatch>,
    ) {
        let metadata = dispatch.filter(|_| code.is_none_or(CueCode::is_reportable));
        let _ = self
            .registry
            .record_transport_audit(crate::node_registry::TransportAudit {
                event_type: event,
                node_id: &self.remote_node_id,
                session_id: Some(&self.session_id),
                direction: None,
                byte_count: 0,
                outcome,
                error_code: code.map(CueCode::code),
                cue: Some(crate::node_registry::CueAudit {
                    id: metadata.map(|dispatch| dispatch.cue_id.as_str()),
                    script: metadata.map(|dispatch| dispatch.script.as_str()),
                    reason: metadata.map(|dispatch| dispatch.reason.as_str()),
                }),
            });
    }

    /// Audit the true code, report the narrowed one, and only to a sender
    /// already authorized to have been evaluated.
    fn refuse(&mut self, dispatch: Option<&CueDispatch>, code: CueCode, now: i64) -> CueOutcome {
        self.audit("cue_rejected", "rejected", Some(code), dispatch);
        if let Some(dispatch) = dispatch.filter(|_| code.is_reportable()) {
            let reported = code.reply_code();
            self.queue_reply(&dispatch.cue_id, Some(reported), now);
        }
        if let Some(dispatch) = dispatch {
            self.remember_cue(&dispatch.cue_id, now);
        }
        CueOutcome::Decided(GateDecision::Rejected(code))
    }

    pub(super) fn prune_cue_state(&mut self, now: i64) {
        let floor = now.saturating_sub(CUE_RETENTION_SECONDS);
        self.cue_records.retain(|record| record.decided_at >= floor);
        while self.cue_records.len() > MAX_RETAINED_CUE_RECORDS {
            self.cue_records.pop_front();
        }
    }

    pub(super) fn remember_cue(&mut self, cue_id: &str, now: i64) {
        self.cue_records.push_back(CueDecisionRecord {
            cue_id: cue_id.to_string(),
            decided_at: now,
            reply: self.pending_reply.clone(),
        });
        while self.cue_records.len() > MAX_RETAINED_CUE_RECORDS {
            self.cue_records.pop_front();
        }
    }

    fn queue_reply(&mut self, cue_id: &str, code: Option<CueCode>, now: i64) {
        let Ok(created_at) = u64::try_from(now) else {
            return;
        };
        let mut nonce = [0u8; 16];
        entropy::fill_bytes(&mut nonce);
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
}
