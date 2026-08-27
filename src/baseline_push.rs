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
//! so before anything goes on the wire. See `.docs/baseline-delivery.md`.

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
    let Ok(Some(mut file)) = context.open_public_file() else {
        return BaselinePolicy::default();
    };
    let mut contents = String::new();
    if std::io::Read::read_to_string(&mut file, &mut contents).is_err() {
        return BaselinePolicy::default();
    }
    let Ok(config) = crate::domain::NodeConfig::parse(&contents) else {
        return BaselinePolicy::default();
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
            return BaselineOutcome::Repeat;
        }

        let Some(workspace) = self.workspace.as_ref() else {
            return self.refuse(Some(baseline_id), BaselineCode::InstallFailed, now);
        };
        // Re-read the sender's standing immediately before the write. The
        // verification above walked a signature and hashed every script; a peer
        // revoked while that ran must not have its code installed.
        if let Err(code) = evaluate_sender_gates(self.policy.enabled, self.authorization().as_ref())
        {
            return self.refuse(Some(baseline_id), code, now);
        }
        match crate::operations::baseline::install_baseline(workspace, &baseline, now as i64) {
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
