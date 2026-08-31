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
use rand::rngs::OsRng;
use rand::RngCore;

/// The two kinds of the baseline plane. There is no third.
pub const KIND_PUSH: &str = "baseline_push";
pub const KIND_ACK: &str = "baseline_ack";

/// The capability a peer must hold to push a baseline.
///
/// Already in the frozen transport allow-list, so nothing about the capability
/// vocabulary changes here.
pub const CAPABILITY_BASELINE_PUSH: &str = "baseline-push";

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
            parse_fixed::<PUBLISHER_ID_BYTES>(&entry.key_id),
            parse_fixed::<PUBLISHER_KEY_BYTES>(&entry.public_key),
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

fn parse_fixed<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut bytes = [0u8; N];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
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
            "manifest": hex(manifest),
            "scripts": bodies.iter().map(|body| hex(body)).collect::<Vec<_>>(),
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
        let manifest = decode_hex(manifest_hex).ok_or(BaselineCode::InvalidMessage)?;
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
            bodies.push(decode_hex(body_hex).ok_or(BaselineCode::InvalidMessage)?);
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len() / 2)
        .map(|index| u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok())
        .collect()
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
        OsRng.fill_bytes(&mut nonce);
        let mut payload = serde_json::json!({
            "version": 1,
            "baseline_id": hex(baseline_id),
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
mod tests {
    use super::*;
    use k256::schnorr::SigningKey;
    use sha2::{Digest, Sha256};

    const ISSUED_AT: u64 = 1_800_000_000;
    const EXPIRES_AT: u64 = 1_800_003_600;

    fn publisher(scalar: u8) -> (SigningKey, BaselinePublisherKey) {
        let signing_key = SigningKey::from_slice(&[scalar; 32]).expect("scalar");
        let mut public_key = [0u8; PUBLISHER_KEY_BYTES];
        public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
        let mut key_id = [0u8; PUBLISHER_ID_BYTES];
        key_id.copy_from_slice(&Sha256::digest(public_key)[..PUBLISHER_ID_BYTES]);
        (
            signing_key,
            BaselinePublisherKey {
                key_id,
                public_key,
                revoked: false,
            },
        )
    }

    fn scripts() -> Vec<(String, Vec<u8>)> {
        vec![
            ("ops/deploy.sh".to_string(), b"echo deploy\n".to_vec()),
            ("audit.py".to_string(), b"print('audit')\n".to_vec()),
        ]
    }

    fn signed(scalar: u8, bodies: &[(String, Vec<u8>)]) -> SignedBaselineManifest {
        let (signing_key, key) = publisher(scalar);
        SignedBaselineManifest::sign_with_material(
            signing_key.to_bytes().as_ref(),
            key.key_id,
            "acme".to_string(),
            bodies,
            ISSUED_AT,
            EXPIRES_AT,
        )
        .expect("sign")
    }

    /// Build the payload the way a sender does: bodies in manifest order.
    fn push_for(manifest: &SignedBaselineManifest, bodies: &[(String, Vec<u8>)]) -> BaselinePush {
        let ordered = manifest
            .entries
            .iter()
            .map(|entry| {
                bodies
                    .iter()
                    .find(|(path, _)| path == &entry.path)
                    .map(|(_, body)| body.clone())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        BaselinePush::parse(&BaselinePush::encode(&manifest.encode(), &ordered)).expect("parse")
    }

    fn policy(scalar: u8) -> BaselinePolicy {
        BaselinePolicy {
            enabled: true,
            publishers: vec![publisher(scalar).1],
            organization: "acme".to_string(),
        }
    }

    /// The claim the module header makes, checked against the real constants
    /// and the real signer rather than against arithmetic in a comment.
    ///
    /// This is the test that would have caught the design being wrong: a
    /// maximal *signable* baseline is 256 MiB, which does not fit in a frozen
    /// 1 MiB frame by any margin, and the answer was a smaller delivery bound
    /// rather than a larger transport one.
    #[test]
    fn a_maximal_push_fits_inside_the_frozen_plaintext_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let identity = test_identity(&dir);
        let bodies: Vec<(String, Vec<u8>)> = (0..8)
            .map(|index| {
                (
                    format!("bulk{index}.sh"),
                    vec![b'x'; MAX_PUSH_SCRIPT_BYTES / 8],
                )
            })
            .collect();
        let manifest = signed(3, &bodies);
        let ordered: Vec<Vec<u8>> = manifest
            .entries
            .iter()
            .map(|entry| {
                bodies
                    .iter()
                    .find(|(path, _)| path == &entry.path)
                    .expect("entry")
                    .1
                    .clone()
            })
            .collect();

        let envelope = crate::direct_transport::sign_baseline_envelope(
            &identity,
            KIND_PUSH,
            &[7u8; 32],
            [9u8; 16],
            BaselinePush::encode(&manifest.encode(), &ordered),
            ISSUED_AT,
        )
        .expect("sign the largest push delivery allows");

        assert!(
            envelope.encoded().len() <= crate::direct_transport::MAX_PLAINTEXT_BYTES,
            "a push at the delivery bound must fit one frame; it was {} of {}",
            envelope.encoded().len(),
            crate::direct_transport::MAX_PLAINTEXT_BYTES
        );
    }

    /// One byte over the bound is refused before anything is allocated for it.
    #[test]
    fn a_push_over_the_delivery_bound_is_refused() {
        let oversized = serde_json::json!({
            "version": 1,
            "manifest": "00",
            "scripts": [hex(&vec![b'x'; MAX_PUSH_SCRIPT_BYTES + 1])],
        });
        assert_eq!(BaselinePush::parse(&oversized), Err(BaselineCode::TooLarge));
    }

    /// The two independent authorities: a trusted sender cannot supply an
    /// untrusted publisher's baseline, and vice versa.
    #[test]
    fn a_baseline_from_a_publisher_this_node_does_not_name_is_refused() {
        let bodies = scripts();
        let manifest = signed(3, &bodies);
        let push = push_for(&manifest, &bodies);

        verify_push(&push, &policy(3), ISSUED_AT + 1)
            .expect("the publisher this node names must be accepted");

        assert_eq!(
            verify_push(&push, &policy(9), ISSUED_AT + 1).err(),
            Some(BaselineCode::PublisherUnknown),
            "a different publisher must not be accepted in the named one's place"
        );

        let empty = BaselinePolicy {
            publishers: Vec::new(),
            ..policy(3)
        };
        assert_eq!(
            verify_push(&push, &empty, ISSUED_AT + 1).err(),
            Some(BaselineCode::PublisherUnknown),
            "naming nobody must accept nobody"
        );

        let revoked = BaselinePolicy {
            publishers: vec![BaselinePublisherKey {
                revoked: true,
                ..publisher(3).1
            }],
            ..policy(3)
        };
        assert_eq!(
            verify_push(&push, &revoked, ISSUED_AT + 1).err(),
            Some(BaselineCode::PublisherRevoked)
        );
    }

    /// The set is what was signed, so one wrong script voids all of it.
    #[test]
    fn a_script_that_does_not_match_its_recorded_hash_voids_the_whole_push() {
        let bodies = scripts();
        let manifest = signed(3, &bodies);
        let mut push = push_for(&manifest, &bodies);

        push.bodies[0].push(b'!');
        assert_eq!(
            verify_push(&push, &policy(3), ISSUED_AT + 1).err(),
            Some(BaselineCode::ContentMismatch)
        );

        let mut short = push_for(&manifest, &bodies);
        short.bodies.pop();
        assert_eq!(
            verify_push(&short, &policy(3), ISSUED_AT + 1).err(),
            Some(BaselineCode::ContentMismatch),
            "a short array must not install a shorter set than the manifest names"
        );

        let mut extra = push_for(&manifest, &bodies);
        extra.bodies.push(b"echo extra\n".to_vec());
        assert_eq!(
            verify_push(&extra, &policy(3), ISSUED_AT + 1).err(),
            Some(BaselineCode::ContentMismatch)
        );
    }

    /// The organization and the validity window are checked against local
    /// facts, not against anything the message asserts about itself.
    #[test]
    fn a_baseline_for_another_organization_or_outside_its_window_is_refused() {
        let bodies = scripts();
        let push = push_for(&signed(3, &bodies), &bodies);

        assert_eq!(
            verify_push(
                &push,
                &BaselinePolicy {
                    organization: "other-org".to_string(),
                    ..policy(3)
                },
                ISSUED_AT + 1,
            )
            .err(),
            Some(BaselineCode::OrganizationMismatch)
        );
        assert_eq!(
            verify_push(&push, &policy(3), EXPIRES_AT).err(),
            Some(BaselineCode::Expired)
        );
    }

    /// Fail-closed, one gate at a time, with a passing control so a refusal
    /// cannot be mistaken for the whole thing being switched off.
    #[test]
    fn each_sender_gate_refuses_on_its_own() {
        let held = vec![CAPABILITY_BASELINE_PUSH.to_string()];
        let authorized = HealthAuthorization {
            node_id: "omk1_test".to_string(),
            state: PeerState::Active,
            role: PeerRole::Conductor,
            capabilities: held.clone(),
        };

        evaluate_sender_gates(true, Some(&authorized)).expect("the passing control");

        assert_eq!(
            evaluate_sender_gates(false, Some(&authorized)),
            Err(BaselineCode::Disabled),
            "the shipped default installs nothing however trusted the sender"
        );
        assert_eq!(
            evaluate_sender_gates(true, None),
            Err(BaselineCode::NotActiveConductor),
            "a peer this node has never heard of must not push code to it"
        );
        assert_eq!(
            evaluate_sender_gates(
                true,
                Some(&HealthAuthorization {
                    role: PeerRole::Performer,
                    ..authorized.clone()
                })
            ),
            Err(BaselineCode::NotActiveConductor)
        );
        assert_eq!(
            evaluate_sender_gates(
                true,
                Some(&HealthAuthorization {
                    state: PeerState::Revoked,
                    ..authorized.clone()
                })
            ),
            Err(BaselineCode::NotActiveConductor),
            "revocation must stop a push, or it is advisory"
        );
        assert_eq!(
            evaluate_sender_gates(
                true,
                Some(&HealthAuthorization {
                    capabilities: vec!["remote-run".to_string()],
                    ..authorized
                })
            ),
            Err(BaselineCode::MissingBaselinePush),
            "ordering a run and supplying what runs are different powers"
        );
    }

    /// What this node thinks of the sender, and whether the feature exists
    /// here at all, are never disclosed. What it thinks of the artefact is.
    #[test]
    fn only_refusals_about_the_sender_are_silent() {
        for silent in [
            BaselineCode::Disabled,
            BaselineCode::NotActiveConductor,
            BaselineCode::MissingBaselinePush,
        ] {
            assert!(
                !silent.is_reportable(),
                "{} must not tell an unauthorized peer anything",
                silent.name()
            );
        }
        for reportable in [
            BaselineCode::PublisherUnknown,
            BaselineCode::PublisherRevoked,
            BaselineCode::OrganizationMismatch,
            BaselineCode::Expired,
            BaselineCode::SignatureMismatch,
            BaselineCode::ContentMismatch,
            BaselineCode::TooLarge,
            BaselineCode::InvalidMessage,
            BaselineCode::InstallFailed,
            BaselineCode::Duplicate,
        ] {
            assert!(
                reportable.is_reportable(),
                "{} is about the artefact the sender chose, and withholding it \
                 leaves an authorized Conductor guessing",
                reportable.name()
            );
        }
    }

    /// Every code is distinct and inside its own band.
    #[test]
    fn the_code_band_is_disjoint_from_every_other_plane() {
        let codes = [
            BaselineCode::Disabled,
            BaselineCode::NotActiveConductor,
            BaselineCode::MissingBaselinePush,
            BaselineCode::InvalidMessage,
            BaselineCode::TooLarge,
            BaselineCode::PublisherUnknown,
            BaselineCode::PublisherRevoked,
            BaselineCode::OrganizationMismatch,
            BaselineCode::Expired,
            BaselineCode::SignatureMismatch,
            BaselineCode::ContentMismatch,
            BaselineCode::InstallFailed,
            BaselineCode::Duplicate,
        ];
        let mut numbers = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for code in codes {
            assert!(
                (1301..=1399).contains(&code.code()),
                "{} escapes the baseline band and could collide with another plane",
                code.name()
            );
            assert!(numbers.insert(code.code()), "{} reuses a code", code.name());
            assert!(names.insert(code.name()), "{} reuses a name", code.name());
        }
    }

    /// A node that cannot prove what it opted into has opted into nothing.
    ///
    /// The control comes first and is load-bearing. Without it every assertion
    /// below would pass just as well if `read_policy` never returned anything
    /// but the default — which is exactly the state this test was in before the
    /// control was added, and it hid that the reader was refusing a perfectly
    /// good config over its file mode.
    #[cfg(unix)]
    #[test]
    fn a_config_that_cannot_be_read_denies_everything() {
        let readable = policy_from(&valid_config(), Some(0o640));
        assert!(readable.enabled, "the passing control");
        assert_eq!(readable.publishers.len(), 1);
        assert_eq!(readable.organization, "acme");

        for (label, text, mode) in [
            (
                "malformed toml",
                "this is not toml = = =".to_string(),
                Some(0o640),
            ),
            (
                "a config the validator refuses",
                valid_config().replace("port = 38383", "port = 1"),
                Some(0o640),
            ),
            (
                "a config anyone on the box can read",
                valid_config(),
                Some(0o644),
            ),
            ("no config at all", String::new(), None),
        ] {
            let policy = policy_from(&text, mode);
            assert!(
                !policy.enabled && policy.publishers.is_empty(),
                "{label} must leave the gate closed and name nobody"
            );
        }
    }

    fn valid_config() -> String {
        let mut config = crate::domain::NodeConfig::default();
        config.trust.enrollment = "manual".to_string();
        config.trust.allow_baseline_push = true;
        config.organization.id = "acme".to_string();
        config.trust.baseline_publishers = vec![crate::domain::TrustedBaselinePublisher {
            key_id: "a".repeat(32),
            public_key: "b".repeat(64),
            revoked: false,
        }];
        config.to_toml().expect("serialize")
    }

    /// `mode = None` writes no config at all, which is a different failure from
    /// writing one that cannot be trusted.
    #[cfg(unix)]
    fn policy_from(text: &str, mode: Option<u32>) -> BaselinePolicy {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("node.toml");
        if let Some(mode) = mode {
            std::fs::write(&config, text).expect("write");
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(mode))
                .expect("chmod");
        }
        let context = crate::node::NodeContext::resolve_for(
            crate::node::NodePlatform::current(),
            crate::node::NodePathOverrides::new(Some(dir.path().join("state")), Some(config)),
            true,
            None,
            None,
            None,
        )
        .expect("context");
        read_policy(&context)
    }

    fn test_identity(dir: &tempfile::TempDir) -> crate::node_identity::NodeIdentity {
        let config = dir.path().join("node.toml");
        std::fs::write(&config, "version = 1\n").expect("write config");
        let context = crate::node::NodeContext::resolve_for(
            crate::node::NodePlatform::current(),
            crate::node::NodePathOverrides::new(Some(dir.path().join("state")), Some(config)),
            true,
            None,
            None,
            None,
        )
        .expect("context");
        crate::node_identity::NodeIdentity::load_or_initialize(&context).expect("identity")
    }
}

/// Baseline delivery, end to end, against the acceptance criteria for item 8.
///
/// Every test here asserts against the **workspace**, not against the reply. A
/// Performer that refuses a baseline is supposed to say very little -- for three
/// of the refusal codes, nothing at all -- so "did it install" can only be
/// answered by looking at the files. A test that read the ack would pass just
/// as well against a node that replied correctly and installed anyway.
///
/// The `BaselineSession` is driven directly rather than through two live node
/// services. The transport underneath is the same code the Cue plane already
/// certified on real sockets, and those tests are `#[ignore]`d because they
/// spawn two processes. What is new here is the gate and the install, and
/// driving the session directly is what makes the *file system* observable at
/// the moment of the decision.
#[cfg(all(test, unix))]
mod delivery_tests {
    use crate::baseline::{BaselinePublisherKey, SignedBaselineManifest};
    use crate::baseline_push::{
        BaselineCode, BaselineOutcome, BaselinePolicy, BaselinePush, BaselineSession,
    };
    use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
    use crate::node_identity::NodeIdentity;
    use crate::node_registry::{NodeRegistry, PeerRole};
    use crate::workspace::Workspace;
    use std::path::Path;

    const ISSUED_AT: u64 = 1_800_000_000;
    const NOW: u64 = 1_800_000_060;
    const LIFETIME: u64 = 3_600;

    /// One node's private state: identity, registry, workspace.
    struct Performer {
        _dir: tempfile::TempDir,
        context: NodeContext,
        workspace: Workspace,
    }

    impl Performer {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let config = dir.path().join("node.toml");
            std::fs::write(&config, "version = 1\n").expect("write config");
            let context = NodeContext::resolve_for(
                NodePlatform::current(),
                NodePathOverrides::new(Some(dir.path().join("state")), Some(config)),
                true,
                None,
                None,
                None,
            )
            .expect("resolve node context");
            let workspace = Workspace::new(dir.path().join("scripts"));
            workspace.ensure_layout().expect("workspace layout");
            Self {
                _dir: dir,
                context,
                workspace,
            }
        }

        fn identity(&self) -> NodeIdentity {
            NodeIdentity::load_or_initialize(&self.context).expect("identity")
        }

        fn registry(&self, identity: &NodeIdentity) -> NodeRegistry {
            NodeRegistry::open_for_initialization(&self.context, identity.public_status())
                .expect("registry")
        }

        fn installed(&self, relative: &str) -> Option<Vec<u8>> {
            std::fs::read(self.workspace.scripts_root().join(relative)).ok()
        }
    }

    /// A publisher the receiver may or may not name.
    fn publisher(scalar: u8) -> (k256::schnorr::SigningKey, BaselinePublisherKey) {
        use sha2::Digest;
        let signing_key = k256::schnorr::SigningKey::from_slice(&[scalar; 32]).expect("scalar");
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
        let mut key_id = [0u8; 16];
        key_id.copy_from_slice(&sha2::Sha256::digest(public_key)[..16]);
        (
            signing_key,
            BaselinePublisherKey {
                key_id,
                public_key,
                revoked: false,
            },
        )
    }

    fn fleet_scripts() -> Vec<(String, Vec<u8>)> {
        vec![
            (
                "ops/deploy.sh".to_string(),
                b"#!/bin/sh\necho fleet-deploy\n".to_vec(),
            ),
            (
                "audit.sh".to_string(),
                b"#!/bin/sh\necho fleet-audit\n".to_vec(),
            ),
        ]
    }

    fn sign(scalar: u8, bodies: &[(String, Vec<u8>)]) -> SignedBaselineManifest {
        let (signing_key, key) = publisher(scalar);
        SignedBaselineManifest::sign_with_material(
            signing_key.to_bytes().as_ref(),
            key.key_id,
            "acme".to_string(),
            bodies,
            ISSUED_AT,
            ISSUED_AT + LIFETIME,
        )
        .expect("sign the baseline")
    }

    /// Build the wire payload the way the sender does: bodies in manifest order.
    fn wire(manifest: &SignedBaselineManifest, bodies: &[(String, Vec<u8>)]) -> serde_json::Value {
        let ordered: Vec<Vec<u8>> = manifest
            .entries
            .iter()
            .map(|entry| {
                bodies
                    .iter()
                    .find(|(path, _)| path == &entry.path)
                    .map(|(_, body)| body.clone())
                    .unwrap_or_default()
            })
            .collect();
        BaselinePush::encode(&manifest.encode(), &ordered)
    }

    fn policy(enabled: bool, publishers: Vec<BaselinePublisherKey>) -> BaselinePolicy {
        BaselinePolicy {
            enabled,
            publishers,
            organization: "acme".to_string(),
        }
    }

    /// Record the sender as an active Conductor holding `baseline-push`.
    fn trust_conductor(registry: &NodeRegistry, node_id: &str, public_key: &str, capability: &str) {
        registry
            .import_manual_peer(crate::node_registry::PeerRegistration {
                node_id: node_id.to_string(),
                public_key: public_key.to_string(),
                role: PeerRole::Conductor,
                capabilities: vec![capability.to_string()],
                source: crate::node_registry::PeerSource::Manual,
                actor: "test".to_string(),
                reason: "baseline delivery test".to_string(),
            })
            .expect("record the conductor");
    }

    /// Drive one push through a real session and report what the workspace holds.
    fn deliver(
        performer: &Performer,
        conductor: &Performer,
        policy: BaselinePolicy,
        payload: serde_json::Value,
    ) -> BaselineOutcome {
        let performer_identity = performer.identity();
        let registry = performer.registry(&performer_identity);
        let conductor_identity = conductor.identity();
        let conductor_status = conductor_identity.public_status();

        trust_conductor(
            &registry,
            &conductor_status.node_id,
            &conductor_status.public_key_hex,
            crate::baseline_push::CAPABILITY_BASELINE_PUSH,
        );
        deliver_with_registry(
            performer,
            &registry,
            &performer_identity,
            &conductor_identity,
            policy,
            payload,
        )
    }

    fn deliver_with_registry(
        performer: &Performer,
        registry: &NodeRegistry,
        performer_identity: &NodeIdentity,
        conductor_identity: &NodeIdentity,
        policy: BaselinePolicy,
        payload: serde_json::Value,
    ) -> BaselineOutcome {
        let session_id = [42u8; 32];
        let mut nonce = [0u8; 16];
        nonce[0] = 7;
        let conductor_status = conductor_identity.public_status();
        let mut conductor_key = [0u8; 32];
        conductor_key.copy_from_slice(
            &(0..32)
                .map(|index| {
                    u8::from_str_radix(
                        &conductor_status.public_key_hex[index * 2..index * 2 + 2],
                        16,
                    )
                    .expect("hex")
                })
                .collect::<Vec<_>>(),
        );

        let envelope = crate::direct_transport::sign_baseline_envelope(
            conductor_identity,
            crate::baseline_push::KIND_PUSH,
            &session_id,
            nonce,
            payload,
            NOW,
        )
        .expect("sign the push");

        let mut session = BaselineSession::new(
            registry,
            performer_identity,
            &conductor_status.node_id,
            conductor_key,
            session_id,
            policy,
            Some(Workspace::new(performer.workspace.root().to_path_buf())),
        );
        session.handle_envelope(&envelope.encoded(), NOW)
    }

    fn accepted(outcome: &BaselineOutcome) -> bool {
        matches!(
            outcome,
            BaselineOutcome::Decided(crate::baseline_push::BaselineDecision::Accepted { .. })
        )
    }

    fn refused_with(outcome: &BaselineOutcome) -> Option<BaselineCode> {
        match outcome {
            BaselineOutcome::Decided(crate::baseline_push::BaselineDecision::Rejected(code)) => {
                Some(*code)
            }
            _ => None,
        }
    }

    /// Acceptance: a Performer with the gate on, naming this publisher, installs
    /// the set, and the scripts are runnable afterwards.
    #[test]
    fn a_named_publishers_baseline_installs_and_the_scripts_run() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);

        let outcome = deliver(
            &performer,
            &conductor,
            policy(true, vec![publisher(3).1]),
            wire(&manifest, &bodies),
        );

        assert!(accepted(&outcome), "got {outcome:?}");
        for (path, body) in &bodies {
            assert_eq!(
                performer.installed(path).as_deref(),
                Some(body.as_slice()),
                "{path} must be on disk with the published bytes"
            );
        }

        // "Runnable" is asserted by running one, not by reading a mode bit: a file
        // whose permissions look right and whose interpreter cannot start it is
        // still not a script anybody can use.
        let installed = performer.workspace.scripts_root().join("audit.sh");
        let output = std::process::Command::new("/bin/sh")
            .arg(&installed)
            .output()
            .expect("run the installed script");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "fleet-audit"
        );
    }

    /// A push repeated on one session must be answered, not met with silence.
    ///
    /// `Duplicate` is reportable and `Repeat` is documented as "answered from
    /// the first decision", but the repeat was audited and then dropped without
    /// queueing an ack. The ack is the only thing a Conductor ever sees, so the
    /// push sat until `--wait-seconds` ran out and then reported
    /// `answered: false` -- byte for byte what a push refused on trust, role,
    /// or capability reports. On two real VMs a duplicate push burned the full
    /// 60-second budget and was indistinguishable from a silently refused one;
    /// telling them apart meant reading the Performer's audit table by hand.
    #[test]
    fn a_repeated_push_is_answered_rather_than_silently_dropped() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);
        let payload = wire(&manifest, &bodies);

        let performer_identity = performer.identity();
        let registry = performer.registry(&performer_identity);
        let conductor_identity = conductor.identity();
        let conductor_status = conductor_identity.public_status();
        trust_conductor(
            &registry,
            &conductor_status.node_id,
            &conductor_status.public_key_hex,
            crate::baseline_push::CAPABILITY_BASELINE_PUSH,
        );

        let session_id = [42u8; 32];
        let mut nonce = [0u8; 16];
        nonce[0] = 7;
        let mut conductor_key = [0u8; 32];
        conductor_key.copy_from_slice(
            &(0..32)
                .map(|index| {
                    u8::from_str_radix(
                        &conductor_status.public_key_hex[index * 2..index * 2 + 2],
                        16,
                    )
                    .expect("hex")
                })
                .collect::<Vec<_>>(),
        );
        let envelope = crate::direct_transport::sign_baseline_envelope(
            &conductor_identity,
            crate::baseline_push::KIND_PUSH,
            &session_id,
            nonce,
            payload,
            NOW,
        )
        .expect("sign the push");

        // One session, deliberately: the duplicate guard is per-session, so a
        // second session would install again rather than exercise the repeat.
        let mut session = BaselineSession::new(
            &registry,
            &performer_identity,
            &conductor_status.node_id,
            conductor_key,
            session_id,
            policy(true, vec![publisher(3).1]),
            Some(Workspace::new(performer.workspace.root().to_path_buf())),
        );

        let first = session.handle_envelope(&envelope.encoded(), NOW);
        assert!(accepted(&first), "the first push must install: {first:?}");
        assert!(
            session.take_reply().is_some(),
            "the first push must be acked"
        );

        let second = session.handle_envelope(&envelope.encoded(), NOW);
        assert_eq!(second, BaselineOutcome::Repeat, "got {second:?}");
        let reply = session.take_reply().expect(
            "a repeated push was audited but never answered: the Conductor is left waiting \
             out its whole budget and cannot tell a duplicate from a silent refusal",
        );
        let view = crate::direct_transport::envelope_view(&reply).expect("decode the ack");
        assert_eq!(view.payload["accepted"], false, "ack: {}", view.payload);
        assert_eq!(
            view.payload["error"]["code"],
            u64::from(BaselineCode::Duplicate.code()),
            "the repeat must name itself as {} rather than any other refusal: {}",
            BaselineCode::Duplicate.name(),
            view.payload
        );
    }

    /// Acceptance: an unnamed publisher changes nothing, and the workspace is what
    /// proves it.
    #[test]
    fn an_unnamed_publisher_changes_nothing_on_disk() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(9, &bodies);

        // Seeded so "nothing changed" is a statement about content, not about
        // absence: a workspace that was empty before and after would pass even if
        // the install had silently been skipped for an unrelated reason.
        std::fs::create_dir_all(performer.workspace.scripts_root().join("ops")).expect("mkdir");
        std::fs::write(
            performer.workspace.scripts_root().join("ops/deploy.sh"),
            b"#!/bin/sh\necho the-operators-own\n",
        )
        .expect("seed");

        let outcome = deliver(
            &performer,
            &conductor,
            policy(true, vec![publisher(3).1]),
            wire(&manifest, &bodies),
        );

        assert_eq!(refused_with(&outcome), Some(BaselineCode::PublisherUnknown));
        assert_eq!(
            performer.installed("ops/deploy.sh").as_deref(),
            Some(b"#!/bin/sh\necho the-operators-own\n".as_slice()),
            "the operator's own script must be untouched"
        );
        assert!(performer.installed("audit.sh").is_none());
    }

    /// Acceptance: a revoked publisher changes nothing either. Revocation that
    /// only affected the reply would be advisory.
    #[test]
    fn a_revoked_publisher_changes_nothing_on_disk() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);
        let revoked = BaselinePublisherKey {
            revoked: true,
            ..publisher(3).1
        };

        let outcome = deliver(
            &performer,
            &conductor,
            policy(true, vec![revoked]),
            wire(&manifest, &bodies),
        );

        assert_eq!(refused_with(&outcome), Some(BaselineCode::PublisherRevoked));
        assert!(performer.installed("ops/deploy.sh").is_none());
        assert!(performer.installed("audit.sh").is_none());
    }

    /// Acceptance: a manifest whose recorded hash does not match its script is
    /// refused as a whole — including the scripts that *did* match.
    #[test]
    fn one_mismatched_script_installs_none_of_the_set() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);

        let mut tampered = bodies.clone();
        tampered
            .iter_mut()
            .find(|(path, _)| path == "ops/deploy.sh")
            .expect("entry")
            .1
            .extend_from_slice(b"echo also-this\n");

        let outcome = deliver(
            &performer,
            &conductor,
            policy(true, vec![publisher(3).1]),
            wire(&manifest, &tampered),
        );

        assert_eq!(refused_with(&outcome), Some(BaselineCode::ContentMismatch));
        assert!(
            performer.installed("audit.sh").is_none(),
            "the script that matched its hash must not survive the set being void"
        );
        assert!(performer.installed("ops/deploy.sh").is_none());
    }

    /// Acceptance: with the gate off — the shipped default — nothing installs and
    /// the node keeps serving.
    ///
    /// The second half is the one `docs/fleet-operations.md` froze for enrollment: refusing
    /// a baseline must never mean refusing to serve. It is checked by delivering a
    /// second baseline on the same session after the refusal and watching it be
    /// decided normally, which a node that had torn the session down could not do.
    #[test]
    fn with_the_gate_off_nothing_installs_and_the_node_keeps_serving() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);

        let performer_identity = performer.identity();
        let registry = performer.registry(&performer_identity);
        let conductor_identity = conductor.identity();
        let conductor_status = conductor_identity.public_status();
        trust_conductor(
            &registry,
            &conductor_status.node_id,
            &conductor_status.public_key_hex,
            crate::baseline_push::CAPABILITY_BASELINE_PUSH,
        );

        let refused = deliver_with_registry(
            &performer,
            &registry,
            &performer_identity,
            &conductor_identity,
            policy(false, vec![publisher(3).1]),
            wire(&manifest, &bodies),
        );
        assert_eq!(refused_with(&refused), Some(BaselineCode::Disabled));
        assert!(performer.installed("ops/deploy.sh").is_none());
        assert!(performer.installed("audit.sh").is_none());

        // The node is still answering: the same session decides a second baseline
        // rather than having stopped serving over the first refusal.
        let still_serving = deliver_with_registry(
            &performer,
            &registry,
            &performer_identity,
            &conductor_identity,
            policy(true, vec![publisher(3).1]),
            wire(&manifest, &bodies),
        );
        assert!(
            accepted(&still_serving),
            "refusing a baseline must not have stopped this node serving; got {still_serving:?}"
        );
    }

    /// A peer this node trusts for runs is not thereby trusted to supply them.
    #[test]
    fn a_conductor_without_the_capability_installs_nothing() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);

        let performer_identity = performer.identity();
        let registry = performer.registry(&performer_identity);
        let conductor_identity = conductor.identity();
        let conductor_status = conductor_identity.public_status();
        trust_conductor(
            &registry,
            &conductor_status.node_id,
            &conductor_status.public_key_hex,
            crate::remote_cue::CAPABILITY_REMOTE_RUN,
        );

        let outcome = deliver_with_registry(
            &performer,
            &registry,
            &performer_identity,
            &conductor_identity,
            policy(true, vec![publisher(3).1]),
            wire(&manifest, &bodies),
        );

        assert_eq!(
            refused_with(&outcome),
            Some(BaselineCode::MissingBaselinePush),
            "ordering a run and supplying what runs are different powers"
        );
        assert!(performer.installed("ops/deploy.sh").is_none());
    }

    /// The sender's standing is decided before the manifest is looked at.
    ///
    /// Ordering is the property, and it is not observable from whether the
    /// install happened -- the standing is re-read before the write too, so
    /// either check alone would keep the workspace clean. What only the *first*
    /// check produces is silence: a peer this node does not trust must be told
    /// nothing about the artefact, not even that its publisher is unknown here.
    /// The refusal code is the evidence of which check ran.
    #[test]
    fn an_untrusted_sender_never_learns_anything_about_the_artefact() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        // Signed by a publisher this node has never named, so the *content*
        // gates have something to say. If they ran, they would say it.
        let manifest = sign(9, &bodies);

        let performer_identity = performer.identity();
        let registry = performer.registry(&performer_identity);
        let conductor_identity = conductor.identity();

        let unknown_peer = deliver_with_registry(
            &performer,
            &registry,
            &performer_identity,
            &conductor_identity,
            policy(true, vec![publisher(3).1]),
            wire(&manifest, &bodies),
        );
        assert_eq!(
            refused_with(&unknown_peer),
            Some(BaselineCode::NotActiveConductor),
            "a peer this node does not know must not receive a verdict on the \
             publisher; that answer belongs to a later gate"
        );
        assert!(
            !BaselineCode::NotActiveConductor.is_reportable(),
            "and the refusal it does get is one it is never told"
        );

        let conductor_status = conductor_identity.public_status();
        trust_conductor(
            &registry,
            &conductor_status.node_id,
            &conductor_status.public_key_hex,
            crate::baseline_push::CAPABILITY_BASELINE_PUSH,
        );
        let trusted_peer = deliver_with_registry(
            &performer,
            &registry,
            &performer_identity,
            &conductor_identity,
            policy(true, vec![publisher(3).1]),
            wire(&manifest, &bodies),
        );
        assert_eq!(
            refused_with(&trusted_peer),
            Some(BaselineCode::PublisherUnknown),
            "the control: once the sender passes, the content gates do answer"
        );
    }

    /// Trust withdrawn during verification stops the write.
    ///
    /// The window is opened deliberately: the baseline is verified while the
    /// peer is trusted, the peer is then revoked, and only then is the install
    /// step reached. In production that gap is the time a signature check and
    /// N content hashes take, which is real but not something a test can pause
    /// inside. Driving the step directly is what makes the check demonstrable
    /// instead of merely present.
    #[test]
    fn a_peer_revoked_during_verification_does_not_get_its_code_installed() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);

        let performer_identity = performer.identity();
        let registry = performer.registry(&performer_identity);
        let conductor_identity = conductor.identity();
        let conductor_status = conductor_identity.public_status();
        trust_conductor(
            &registry,
            &conductor_status.node_id,
            &conductor_status.public_key_hex,
            crate::baseline_push::CAPABILITY_BASELINE_PUSH,
        );

        let policy = policy(true, vec![publisher(3).1]);
        let push = BaselinePush::parse(&wire(&manifest, &bodies)).expect("parse");
        let verified =
            crate::baseline_push::verify_push(&push, &policy, NOW).expect("verified while trusted");
        let baseline_id = verified.baseline_id().expect("id");

        let mut session = BaselineSession::new(
            &registry,
            &performer_identity,
            &conductor_status.node_id,
            [0u8; 32],
            [42u8; 32],
            policy,
            Some(Workspace::new(performer.workspace.root().to_path_buf())),
        );

        // The control: still trusted, so this step does install. Without it a
        // refusal below would prove only that the step never writes anything.
        assert!(accepted(&session.install_when_still_trusted(
            &verified,
            baseline_id,
            NOW
        )));
        std::fs::remove_file(performer.workspace.scripts_root().join("audit.sh"))
            .expect("clear the control");

        registry
            .revoke_peer(
                &conductor_status.node_id,
                "test",
                "revoked mid-verification",
            )
            .expect("revoke");

        let outcome = session.install_when_still_trusted(&verified, baseline_id, NOW);

        assert_eq!(
            refused_with(&outcome),
            Some(BaselineCode::NotActiveConductor),
            "revocation must stop work already in flight, or it is advisory"
        );
        assert!(
            performer.installed("audit.sh").is_none(),
            "the revoked peer's code must not have reached the disk"
        );
    }

    /// The wire carries no paths, so a sender cannot redirect one script.
    ///
    /// There is no field in the payload in which to say where a script goes; the
    /// manifest is the only source. This checks the consequence rather than the
    /// absence: an extra body is refused because the manifest names two entries,
    /// and nothing about the wire can change what those two entries are.
    #[test]
    fn the_wire_cannot_name_a_destination_the_publisher_did_not_sign() {
        let performer = Performer::new();
        let conductor = Performer::new();
        let bodies = fleet_scripts();
        let manifest = sign(3, &bodies);

        let mut payload = wire(&manifest, &bodies);
        payload["scripts"]
            .as_array_mut()
            .expect("scripts array")
            .push(serde_json::json!("23"));

        let outcome = deliver(
            &performer,
            &conductor,
            policy(true, vec![publisher(3).1]),
            payload,
        );

        assert_eq!(refused_with(&outcome), Some(BaselineCode::ContentMismatch));
        assert!(performer.installed("ops/deploy.sh").is_none());
    }

    /// The publish CLI refuses a baseline it knows can never be delivered.
    #[test]
    fn publishing_more_than_one_push_can_carry_is_refused_at_signing_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Workspace::new(dir.path().to_path_buf());
        workspace.ensure_layout().expect("layout");
        std::fs::write(
            workspace.scripts_root().join("huge.sh"),
            vec![b'x'; crate::baseline_push::MAX_PUSH_SCRIPT_BYTES + 1],
        )
        .expect("write");

        let key_dir = tempfile::tempdir().expect("tempdir");
        let config = key_dir.path().join("node.toml");
        std::fs::write(&config, "version = 1\n").expect("write config");
        let context = NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(Some(key_dir.path().join("state")), Some(config)),
            true,
            None,
            None,
            None,
        )
        .expect("context");
        let identity = NodeIdentity::load_or_initialize(&context).expect("identity");
        let registry = NodeRegistry::open_for_initialization(&context, identity.public_status())
            .expect("registry");
        let publisher =
            crate::baseline_publisher::BaselinePublisher::create(&context, &registry).expect("key");

        let error = crate::operations::baseline::publish_baseline(
            &workspace,
            &publisher,
            "acme",
            &["huge.sh".to_string()],
            ISSUED_AT,
            LIFETIME,
            Path::new(&dir.path().join("manifest.ombm")),
        )
        .expect_err("a baseline that cannot be delivered must not be signed");

        assert!(
            error.to_string().contains("cannot be delivered"),
            "got {error:?}"
        );
        assert!(
            !dir.path().join("manifest.ombm").exists(),
            "a refused publish must not leave a manifest behind"
        );
    }
}
