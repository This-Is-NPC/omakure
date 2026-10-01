//! The receive half of baseline delivery: deciding whether a node will let
//! another node put code on it.
//!
//! Every previous plane could be argued about in terms of what an attacker
//! would need to already possess. This one cannot be argued that way, because
//! a baseline is the first message on this transport that *carries code*. The
//! worst outcome of a compromised Cue was running something the owner had
//! already put in the workspace; the worst outcome here is arbitrary code. So
//! the gates are not a refinement of the Cue's — they are the Cue's plus a
//! second, independent authority: the sender must be a trusted Conductor *and*
//! the bytes must be signed by a publisher this node named, and neither
//! substitutes for the other. Compromising the Conductor's session key does
//! not produce a signature; holding the publisher key does not produce a
//! session.
//!
//! Everything the gates read is local — this node's own config, its own
//! registry, its own clock. No field of the inbound message contributes to the
//! decision to accept it. The manifest is the *subject* of the decision, never
//! an input to it.
//!
//! **One envelope, and a bound that says so.** The frozen Noise plaintext limit
//! is `MAX_PLAINTEXT_BYTES` (1,048,520). A manifest may be 64 KiB and the
//! signable set may hold 256 scripts of 1 MiB each, so a *maximal* baseline is
//! roughly 256 MiB and does not fit — not in one frame, and not by any margin.
//! Rather than raise a frozen transport bound or invent a chunked reassembly
//! protocol with its own buffering and abort states, delivery carries its own
//! smaller limit: [`MAX_PUSH_SCRIPT_BYTES`] of script content per push,
//! enforced on both sides. A baseline larger than that is still signable and
//! still installable locally; it is simply not pushable, and the sender is told
//! so before anything goes on the wire. See `docs/internal/baseline-delivery.md`.

use crate::baseline::{
    BaselineError, BaselinePublisherKey, SignedBaselineManifest, VerifiedBaseline,
    BASELINE_ID_BYTES, MAX_ENTRIES, MAX_MANIFEST_BYTES, PUBLISHER_ID_BYTES, PUBLISHER_KEY_BYTES,
};
use crate::node_registry::health::HealthAuthorization;
use crate::node_registry::{PeerRole, PeerState};
use crate::util::entropy;
use crate::util::hex;

/// The two kinds of the baseline plane. There is no third.
pub const KIND_PUSH: &str = "baseline_push";
pub const KIND_ACK: &str = "baseline_ack";

/// The capability a peer must hold to push a baseline.
pub use crate::domain::CAPABILITY_BASELINE_PUSH;

/// The most script content one push may carry, in raw bytes before hex.
///
/// Chosen so a maximal push is comfortably inside the frozen plaintext limit
/// rather than exactly at it: hex doubles the content, the manifest may add
/// 64 KiB (128 KiB hexed), and the envelope and JSON scaffolding add a few
/// hundred bytes more. See `push_size_bound_fits_the_frozen_plaintext_limit`,
/// which does the arithmetic against the real constants rather than trusting
/// this comment.
pub const MAX_PUSH_SCRIPT_BYTES: usize = 256 * 1024;

/// The premise this bound exists for, wired to the compiler.
///
/// If a maximal *signable* baseline ever fit inside one frame, the delivery
/// bound above would be an arbitrary restriction rather than a consequence, and
/// it should be removed rather than left standing with a stale rationale. This
/// breaks the build the day that changes.
const _: () = assert!(
    MAX_ENTRIES * crate::baseline::MAX_SCRIPT_BYTES > crate::direct_transport::MAX_PLAINTEXT_BYTES
);

/// Stable rejection codes, in a band disjoint from transport (`1001..=1020`),
/// Health (`1101..=1115`) and Cue (`1201..=1212`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineCode {
    Disabled,
    NotActiveConductor,
    MissingBaselinePush,
    InvalidMessage,
    TooLarge,
    PublisherUnknown,
    PublisherRevoked,
    OrganizationMismatch,
    Expired,
    SignatureMismatch,
    ContentMismatch,
    InstallFailed,
    Duplicate,
}

impl BaselineCode {
    pub fn code(self) -> u16 {
        match self {
            BaselineCode::Disabled => 1301,
            BaselineCode::NotActiveConductor => 1302,
            BaselineCode::MissingBaselinePush => 1303,
            BaselineCode::InvalidMessage => 1304,
            BaselineCode::TooLarge => 1305,
            BaselineCode::PublisherUnknown => 1306,
            BaselineCode::PublisherRevoked => 1307,
            BaselineCode::OrganizationMismatch => 1308,
            BaselineCode::Expired => 1309,
            BaselineCode::SignatureMismatch => 1310,
            BaselineCode::ContentMismatch => 1311,
            BaselineCode::InstallFailed => 1312,
            BaselineCode::Duplicate => 1313,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BaselineCode::Disabled => "baseline_disabled",
            BaselineCode::NotActiveConductor => "baseline_not_active_conductor",
            BaselineCode::MissingBaselinePush => "baseline_missing_baseline_push",
            BaselineCode::InvalidMessage => "baseline_invalid_message",
            BaselineCode::TooLarge => "baseline_too_large",
            BaselineCode::PublisherUnknown => "baseline_publisher_unknown",
            BaselineCode::PublisherRevoked => "baseline_publisher_revoked",
            BaselineCode::OrganizationMismatch => "baseline_organization_mismatch",
            BaselineCode::Expired => "baseline_expired",
            BaselineCode::SignatureMismatch => "baseline_signature_mismatch",
            BaselineCode::ContentMismatch => "baseline_content_mismatch",
            BaselineCode::InstallFailed => "baseline_install_failed",
            BaselineCode::Duplicate => "baseline_duplicate",
        }
    }

    /// Whether a refusal with this code may be told to the sender.
    ///
    /// The Health and Cue precedent, unchanged: whether this node has the
    /// feature on, and what it thinks of the *sender*, are never disclosed —
    /// an unauthorized peer must not learn that baseline push exists here.
    /// Everything else is about the artefact the sender chose to send, and a
    /// Conductor already authorized to push needs to know why its push did not
    /// land or it can only guess.
    pub fn is_reportable(self) -> bool {
        !matches!(
            self,
            BaselineCode::Disabled
                | BaselineCode::NotActiveConductor
                | BaselineCode::MissingBaselinePush
        )
    }
}

/// Everything the gates read, all of it local to the receiver.
///
/// There is deliberately no way to build one from an inbound payload.
#[derive(Debug, Clone, Default)]
pub struct BaselinePolicy {
    /// `trust.allow_baseline_push` from this node's own config.
    pub enabled: bool,
    /// `trust.baseline_publishers`. Empty means nobody, which is the shipped
    /// state and the state any failure to read the config falls back to.
    pub publishers: Vec<BaselinePublisherKey>,
    /// `organization.id`, which the manifest must match.
    pub organization: String,
}

/// Read the baseline policy from this node's own configuration.
///
/// Read per session rather than cached at service start, so revoking a
/// publisher or closing the gate takes effect on the next session instead of
/// requiring a restart. Any failure to read yields the default, which denies
/// everything: a node that cannot prove what it opted into has opted into
/// nothing.
pub fn read_policy(context: &crate::node::NodeContext) -> BaselinePolicy {
    let config = match crate::node::read_policy_config(context) {
        crate::node::PolicyConfig::Declared(config) => *config,
        // Nothing declared is the shipped state and needs no comment.
        crate::node::PolicyConfig::NothingDeclared => return BaselinePolicy::default(),
        // Same decision as "nothing declared", entirely different operator
        // problem. Say which one it was.
        crate::node::PolicyConfig::Unreadable(reason) => {
            crate::node::warn_policy_unreadable("baseline push", &reason);
            return BaselinePolicy::default();
        }
    };
    let mut publishers = Vec::with_capacity(config.trust.baseline_publishers.len());
    for entry in &config.trust.baseline_publishers {
        // A malformed entry is dropped rather than failing the whole read. The
        // alternative would let one bad line silently disable a gate the
        // operator believes is on for every *other* publisher, and validation
        // already refuses to load a config containing one.
        let (Some(key_id), Some(public_key)) = (
            hex::decode_array::<PUBLISHER_ID_BYTES>(&entry.key_id),
            hex::decode_array::<PUBLISHER_KEY_BYTES>(&entry.public_key),
        ) else {
            continue;
        };
        publishers.push(BaselinePublisherKey {
            key_id,
            public_key,
            revoked: entry.revoked,
        });
    }
    BaselinePolicy {
        enabled: config.trust.allow_baseline_push,
        publishers,
        organization: config.organization.id,
    }
}

/// The gate decision. `Accepted` means the set is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineDecision {
    Accepted {
        baseline_id: [u8; BASELINE_ID_BYTES],
    },
    Rejected(BaselineCode),
}

/// What the dispatcher should do with an inbound frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineOutcome {
    /// Not baseline traffic; the dispatcher keeps its existing behaviour.
    NotBaseline,
    /// Decided and audited for the first time on this session.
    Decided(BaselineDecision),
    /// A baseline already decided on this session, answered from the first
    /// decision rather than installed a second time.
    Repeat,
}

/// The three gates that read only the sender's standing with this node.
///
/// Evaluated before the manifest is even decoded, and in this order, so a node
/// with the gate closed produces the same silence for every peer regardless of
/// what it knows about them — and so no code path can reach a signature
/// verification on behalf of a peer it does not trust.
pub fn evaluate_sender_gates(
    enabled: bool,
    authorization: Option<&HealthAuthorization>,
) -> Result<(), BaselineCode> {
    if !enabled {
        return Err(BaselineCode::Disabled);
    }
    let Some(authorization) = authorization else {
        return Err(BaselineCode::NotActiveConductor);
    };
    if authorization.role != PeerRole::Conductor || authorization.state != PeerState::Active {
        return Err(BaselineCode::NotActiveConductor);
    }
    if !authorization
        .capabilities
        .iter()
        .any(|held| held == CAPABILITY_BASELINE_PUSH)
    {
        return Err(BaselineCode::MissingBaselinePush);
    }
    Ok(())
}

/// Find the publisher this node records for a manifest's key id.
///
/// A miss is `PublisherUnknown` rather than a fallback to anything: a node
/// that names no publisher accepts no baseline, which is the shipped state.
pub fn named_publisher<'a>(
    publishers: &'a [BaselinePublisherKey],
    key_id: &[u8; PUBLISHER_ID_BYTES],
) -> Result<&'a BaselinePublisherKey, BaselineCode> {
    publishers
        .iter()
        .find(|publisher| &publisher.key_id == key_id)
        .ok_or(BaselineCode::PublisherUnknown)
}

/// Map a verification failure onto the wire code for it.
pub fn map_error(error: BaselineError) -> BaselineCode {
    match error {
        BaselineError::Invalid => BaselineCode::InvalidMessage,
        BaselineError::TooLarge => BaselineCode::TooLarge,
        BaselineError::Expired => BaselineCode::Expired,
        BaselineError::PublisherUnknown => BaselineCode::PublisherUnknown,
        BaselineError::PublisherRevoked => BaselineCode::PublisherRevoked,
        BaselineError::OrganizationMismatch => BaselineCode::OrganizationMismatch,
        BaselineError::SignatureMismatch => BaselineCode::SignatureMismatch,
        BaselineError::ContentMismatch => BaselineCode::ContentMismatch,
    }
}

/// The `baseline_push` payload, after shape validation.
///
/// Scripts travel as an ordered array of hex bodies with **no paths on the
/// wire**. The paths come from the signed manifest and nowhere else, so the
/// sender cannot name a destination the publisher did not sign — not even one
/// that would fail a later check, because there is no field in which to say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselinePush {
    pub manifest: Vec<u8>,
    pub bodies: Vec<Vec<u8>>,
}

impl BaselinePush {
    /// Build the payload one push carries.
    pub fn encode(manifest: &[u8], bodies: &[Vec<u8>]) -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "manifest": hex::encode(manifest),
            "scripts": bodies.iter().map(|body| hex::encode(body)).collect::<Vec<_>>(),
        })
    }

    /// Parse and bound-check, never trusting a declared length.
    ///
    /// Every bound is applied before the bytes are decoded, so an oversized
    /// push costs a length comparison rather than an allocation proportional
    /// to what it claims to be.
    pub fn parse(payload: &serde_json::Value) -> Result<Self, BaselineCode> {
        let object = payload.as_object().ok_or(BaselineCode::InvalidMessage)?;
        if object.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
            return Err(BaselineCode::InvalidMessage);
        }
        let manifest_hex = object
            .get("manifest")
            .and_then(serde_json::Value::as_str)
            .ok_or(BaselineCode::InvalidMessage)?;
        if manifest_hex.len() > MAX_MANIFEST_BYTES * 2 {
            return Err(BaselineCode::TooLarge);
        }
        let manifest = hex::decode(manifest_hex).ok_or(BaselineCode::InvalidMessage)?;
        let entries = object
            .get("scripts")
            .and_then(serde_json::Value::as_array)
            .ok_or(BaselineCode::InvalidMessage)?;
        if entries.len() > MAX_ENTRIES {
            return Err(BaselineCode::TooLarge);
        }
        let mut total = 0usize;
        let mut bodies = Vec::with_capacity(entries.len());
        for entry in entries {
            let body_hex = entry.as_str().ok_or(BaselineCode::InvalidMessage)?;
            total = total
                .checked_add(body_hex.len() / 2)
                .ok_or(BaselineCode::TooLarge)?;
            if total > MAX_PUSH_SCRIPT_BYTES {
                return Err(BaselineCode::TooLarge);
            }
            bodies.push(hex::decode(body_hex).ok_or(BaselineCode::InvalidMessage)?);
        }
        Ok(Self { manifest, bodies })
    }
}

/// Verify a push end to end against locally-read facts, or refuse it.
///
/// Returns a [`VerifiedBaseline`], which only `bind` can construct, so a caller
/// holding one has the whole set checked against a signature it trusts. Nothing
/// here writes anything: deciding and installing stay separable so each can be
/// reviewed on its own.
pub fn verify_push(
    push: &BaselinePush,
    policy: &BaselinePolicy,
    now: u64,
) -> Result<VerifiedBaseline, BaselineCode> {
    let manifest = SignedBaselineManifest::decode(&push.manifest).map_err(map_error)?;
    let publisher = named_publisher(&policy.publishers, &manifest.publisher_key_id)?;
    manifest
        .verify(publisher, &policy.organization, now)
        .map_err(map_error)?;

    // The count is checked here rather than left to `bind`, because the pairs
    // below are zipped: a short array would otherwise silently produce a
    // shorter set than the manifest describes.
    if push.bodies.len() != manifest.entries.len() {
        return Err(BaselineCode::ContentMismatch);
    }
    let scripts: Vec<(String, Vec<u8>)> = manifest
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .zip(push.bodies.iter().cloned())
        .collect();
    manifest.bind(scripts).map_err(map_error)
}

/// The receive-side baseline session.
///
/// Constructed beside the Health and Cue sessions from the same session facts,
/// and it borrows the registry rather than owning a channel to anything.
pub struct BaselineSession<'a> {
    registry: &'a crate::node_registry::NodeRegistry,
    identity: &'a crate::node_identity::NodeIdentity,
    /// The sender's identity key, as the *handshake* established it.
    remote_identity_key: [u8; 32],
    /// The transport session a push must belong to, so a captured push cannot
    /// be replayed onto a new connection.
    session_id: [u8; 32],
    remote_node_id: String,
    policy: BaselinePolicy,
    /// The workspace a baseline installs into.
    ///
    /// `None` means decide and audit but never install: a node with no
    /// workspace has nowhere to put scripts and should say so.
    workspace: Option<crate::workspace::Workspace>,
    /// Baseline ids already decided on this session, so a retransmission is
    /// answered from the first decision rather than reinstalled.
    seen: std::collections::HashSet<[u8; BASELINE_ID_BYTES]>,
    pending_reply: Option<Vec<u8>>,
}

impl<'a> BaselineSession<'a> {
    pub fn new(
        registry: &'a crate::node_registry::NodeRegistry,
        identity: &'a crate::node_identity::NodeIdentity,
        remote_node_id: &str,
        remote_identity_key: [u8; 32],
        session_id: [u8; 32],
        policy: BaselinePolicy,
        workspace: Option<crate::workspace::Workspace>,
    ) -> Self {
        Self {
            registry,
            identity,
            remote_identity_key,
            session_id,
            remote_node_id: remote_node_id.to_string(),
            policy,
            workspace,
            seen: std::collections::HashSet::new(),
            pending_reply: None,
        }
    }

    /// Decide one inbound envelope, end to end.
    ///
    /// Returns `NotBaseline` for anything outside the `baseline_` namespace so
    /// the dispatcher's existing fall-through is preserved exactly.
    pub fn handle_envelope(&mut self, encoded: &[u8], now: u64) -> BaselineOutcome {
        let Some(kind) = crate::direct_transport::envelope_kind_hint(encoded) else {
            return BaselineOutcome::NotBaseline;
        };
        if !kind.starts_with(crate::direct_transport::BASELINE_KIND_PREFIX) {
            return BaselineOutcome::NotBaseline;
        }
        // A `baseline_ack` is the Conductor's half. A Performer receiving one
        // has been sent a message for the other direction.
        if kind != KIND_PUSH {
            return self.refuse(None, BaselineCode::InvalidMessage, now);
        }

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
            return self.refuse(None, BaselineCode::InvalidMessage, now);
        }

        // The sender's standing is decided before the manifest is looked at,
        // so an untrusted peer never reaches a signature verification and
        // never learns whether this node would have liked its publisher.
        if let Err(code) = evaluate_sender_gates(self.policy.enabled, self.authorization().as_ref())
        {
            return self.refuse(None, code, now);
        }

        let Ok(view) = crate::direct_transport::envelope_view(encoded) else {
            return self.refuse(None, BaselineCode::InvalidMessage, now);
        };
        let push = match BaselinePush::parse(&view.payload) {
            Ok(push) => push,
            Err(code) => return self.refuse(None, code, now),
        };
        let baseline = match verify_push(&push, &self.policy, now) {
            Ok(baseline) => baseline,
            Err(code) => return self.refuse(self.peek_id(&push), code, now),
        };
        let Ok(baseline_id) = baseline.baseline_id() else {
            return self.refuse(None, BaselineCode::InvalidMessage, now);
        };

        if !self.seen.insert(baseline_id) {
            self.audit(
                "baseline_rejected",
                "rejected",
                Some(BaselineCode::Duplicate),
            );
            // Answered from the first decision, which is what `Repeat` means.
            // `Duplicate` is reportable, and the ack is the only thing the
            // sender ever sees: with nothing queued here the Conductor waits
            // out its whole `--wait-seconds` budget and then reports the same
            // `answered: false` that a push refused on trust, role, or
            // capability produces. Those are opposite facts to an operator.
            self.queue_reply(&baseline_id, Some(BaselineCode::Duplicate), now);
            return BaselineOutcome::Repeat;
        }

        self.install_when_still_trusted(&baseline, baseline_id, now)
    }

    /// Re-read the sender's standing, then write.
    ///
    /// A method rather than four inline lines so the window it guards can be
    /// opened deliberately in a test. Verification above walked a signature and
    /// hashed every script; a peer revoked while that ran must not have its
    /// code installed, and a check that only ever runs microseconds after the
    /// first one is a check nothing can demonstrate.
    fn install_when_still_trusted(
        &mut self,
        baseline: &VerifiedBaseline,
        baseline_id: [u8; BASELINE_ID_BYTES],
        now: u64,
    ) -> BaselineOutcome {
        if let Err(code) = evaluate_sender_gates(self.policy.enabled, self.authorization().as_ref())
        {
            return self.refuse(Some(baseline_id), code, now);
        }
        let installed = match self.workspace.as_ref() {
            Some(workspace) => {
                crate::operations::baseline::install_baseline(workspace, baseline, now as i64)
                    .map(|_| ())
            }
            // A node with no workspace has nowhere to put scripts and says so.
            None => Err(crate::operations::OperationError::new(
                crate::operations::OperationErrorCode::NotFound,
                "this node has no workspace to install a baseline into",
            )),
        };
        match installed {
            Ok(_) => {
                self.audit("baseline_installed", "accepted", None);
                self.queue_reply(&baseline_id, None, now);
                BaselineOutcome::Decided(BaselineDecision::Accepted { baseline_id })
            }
            Err(_) => self.refuse(Some(baseline_id), BaselineCode::InstallFailed, now),
        }
    }

    /// The baseline id of a push whose manifest decodes, for the reply only.
    ///
    /// A refusal should name what it refused where it can, but a manifest that
    /// does not decode has no name, and inventing one would let the reply echo
    /// something the sender chose rather than something this node computed.
    fn peek_id(&self, push: &BaselinePush) -> Option<[u8; BASELINE_ID_BYTES]> {
        SignedBaselineManifest::decode(&push.manifest)
            .ok()
            .and_then(|manifest| manifest.baseline_id().ok())
    }

    fn authorization(&self) -> Option<HealthAuthorization> {
        self.registry
            .health_authorization(&self.remote_node_id)
            .ok()
            .flatten()
    }

    fn audit(&self, event: &str, outcome: &str, code: Option<BaselineCode>) {
        let _ = self.registry.record_transport_audit(
            event,
            &self.remote_node_id,
            Some(&self.session_id),
            None,
            0,
            outcome,
            code.map(BaselineCode::code),
        );
    }

    fn refuse(
        &mut self,
        baseline_id: Option<[u8; BASELINE_ID_BYTES]>,
        code: BaselineCode,
        now: u64,
    ) -> BaselineOutcome {
        self.audit("baseline_rejected", "rejected", Some(code));
        if code.is_reportable() {
            if let Some(baseline_id) = baseline_id {
                self.queue_reply(&baseline_id, Some(code), now);
            }
        }
        BaselineOutcome::Decided(BaselineDecision::Rejected(code))
    }

    fn queue_reply(
        &mut self,
        baseline_id: &[u8; BASELINE_ID_BYTES],
        code: Option<BaselineCode>,
        now: u64,
    ) {
        let mut nonce = [0u8; 16];
        entropy::fill_bytes(&mut nonce);
        let mut payload = serde_json::json!({
            "version": 1,
            "baseline_id": hex::encode(baseline_id),
            "accepted": code.is_none(),
        });
        if let Some(code) = code {
            payload["error"] = serde_json::json!({ "code": code.code() });
        }
        self.pending_reply = crate::direct_transport::sign_baseline_envelope(
            self.identity,
            KIND_ACK,
            &self.session_id,
            nonce,
            payload,
            now,
        )
        .ok()
        .map(|envelope| envelope.encoded());
    }

    /// The signed `baseline_ack` this session owes the sender, if any.
    pub fn take_reply(&mut self) -> Option<Vec<u8>> {
        self.pending_reply.take()
    }
}

#[cfg(test)]
mod tests;
