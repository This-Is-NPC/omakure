use super::envelope::{sign_envelope, SignedEnvelope};
use super::errors::TransportError;
use crate::node_identity::NodeIdentity;
use crate::util::hex;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Health Plane carriage
//
// The Health Plane adds envelope `kind` values only. It reuses `sign_envelope`
// verbatim, so the frozen BIP-340 construction, the RFC-8785 canonical prehash,
// the certificate, the Noise handshake, and the framing are all unchanged.
// See `docs/internal/health-plane-contract.md` "Production carriage feasibility".
// ---------------------------------------------------------------------------

/// The `kind` prefix that marks an envelope as a Health Plane message.
pub const HEALTH_KIND_PREFIX: &str = "health_";

/// The Remote Cue kind namespace, disjoint from the Health Plane's.
pub const CUE_KIND_PREFIX: &str = "cue_";

/// The baseline delivery kind namespace, disjoint from both of the above.
pub const BASELINE_KIND_PREFIX: &str = "baseline_";

/// Bytes scanned when reading `kind` without parsing the document.
///
/// RFC-8785 sorts the seven frozen envelope keys, so `kind` always precedes
/// `payload`. A bounded prefix scan therefore reads the real top-level `kind`
/// before any attacker-controlled body, which lets the receiver apply the
/// frozen per-kind size cap *before* JSON parsing allocates anything
/// proportional to the declared content.
const KIND_SCAN_LIMIT: usize = 256;

/// Read the envelope `kind` without parsing the envelope.
///
/// Returns `None` when the prefix does not contain a syntactically plausible
/// `kind`. A hint that disagrees with the real top-level `kind` cannot be
/// exploited: `verify_envelope` re-encodes canonically and compares `kind`
/// against the same expectation, so a mismatch is a bounded rejection.
pub fn envelope_kind_hint(encoded: &[u8]) -> Option<&str> {
    if encoded.len() < 64 {
        return None;
    }
    let canonical = &encoded[..encoded.len() - 64];
    let window = &canonical[..canonical.len().min(KIND_SCAN_LIMIT)];
    let marker = b"\"kind\":\"";
    let start = window
        .windows(marker.len())
        .position(|candidate| candidate == marker)?
        + marker.len();
    let rest = window.get(start..)?;
    let end = rest.iter().position(|byte| *byte == b'"')?;
    if end > MAX_ENVELOPE_KIND_BYTES {
        return None;
    }
    std::str::from_utf8(&rest[..end]).ok()
}

/// The longest envelope `kind` the shipped protocol defines.
const MAX_ENVELOPE_KIND_BYTES: usize = 32;

/// The read-only view a receiver needs after `verify_envelope`.
///
/// Plane-agnostic on purpose: both the Health Plane and the Cue plane read the
/// same two fields out of the same frozen envelope. Naming it for one plane
/// would mean the other reads through a helper named for rules that did not
/// apply to it, which is how a reviewer loses track of which check ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeView {
    /// The envelope `created_at`, in UTC Unix seconds.
    pub created_at: i64,
    /// The envelope `payload` object.
    pub payload: Value,
}

/// Read `created_at` and `payload` out of an already-verified envelope.
///
/// This is a read-only projection. It performs no signature, session, or
/// authorization work: `verify_envelope` owns all of that and must be called
/// first, and each plane owns everything after it.
pub fn envelope_view(encoded: &[u8]) -> Result<EnvelopeView, TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let value: Value = serde_json::from_slice(&encoded[..encoded.len() - 64])
        .map_err(|_| TransportError::InvalidFrame)?;
    let object = value.as_object().ok_or(TransportError::InvalidFrame)?;
    let created_at = object
        .get("created_at")
        .and_then(Value::as_i64)
        .ok_or(TransportError::InvalidFrame)?;
    let payload = object
        .get("payload")
        .cloned()
        .ok_or(TransportError::InvalidFrame)?;
    Ok(EnvelopeView {
        created_at,
        payload,
    })
}

/// Sign one Health Plane message with the frozen envelope construction.
///
/// The only thing this adds over `sign_probe` and its siblings is the `kind`
/// string and the payload object; the signing construction itself is untouched.
/// Kinds outside the closed Health Plane set are refused here so this wrapper
/// can never become a generic envelope-signing oracle.
pub fn sign_health_envelope(
    identity: &NodeIdentity,
    kind: &str,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    payload: Value,
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    if !kind.starts_with(HEALTH_KIND_PREFIX) || kind.len() > MAX_ENVELOPE_KIND_BYTES {
        return Err(TransportError::InvalidFrame);
    }
    if !payload.is_object() {
        return Err(TransportError::InvalidFrame);
    }
    sign_envelope(identity, kind, session_id, nonce, payload, now)
}

/// The Remote Cue signer, sibling to `sign_health_envelope`.
///
/// Both wrap the same private, kind-agnostic `sign_envelope`, and both refuse a
/// kind outside their own namespace. That symmetry is the point: neither
/// wrapper can be used to sign for the other's plane, so a bug or a future
/// caller in one cannot become a signing oracle for the other.
///
/// The envelope, its domain, the signature construction, and the inner frame
/// are all unchanged. A Cue is new traffic over frozen carriage.
pub fn sign_cue_envelope(
    identity: &NodeIdentity,
    kind: &str,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    payload: Value,
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    if !kind.starts_with(CUE_KIND_PREFIX) || kind.len() > MAX_ENVELOPE_KIND_BYTES {
        return Err(TransportError::InvalidFrame);
    }
    if !payload.is_object() {
        return Err(TransportError::InvalidFrame);
    }
    sign_envelope(identity, kind, session_id, nonce, payload, now)
}

/// The baseline delivery signer, third sibling to the other two.
///
/// Adds a `kind` and a payload to the frozen envelope and nothing else. The
/// domain, the BIP-340 construction, the RFC-8785 prehash, the Noise session
/// and the inner frame are all untouched — a baseline is new traffic over
/// frozen carriage, exactly as a Cue was.
///
/// Refuses any kind outside `baseline_` for the reason the other two do: three
/// wrappers over one private signer are only a separation if none of them can
/// be used to sign for another's plane.
pub fn sign_baseline_envelope(
    identity: &NodeIdentity,
    kind: &str,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    payload: Value,
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    if !kind.starts_with(BASELINE_KIND_PREFIX) || kind.len() > MAX_ENVELOPE_KIND_BYTES {
        return Err(TransportError::InvalidFrame);
    }
    if !payload.is_object() {
        return Err(TransportError::InvalidFrame);
    }
    sign_envelope(identity, kind, session_id, nonce, payload, now)
}

pub fn enrollment_request_bytes(encoded: &[u8]) -> Result<Vec<u8>, TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let canonical = &encoded[..encoded.len() - 64];
    let value: Value =
        serde_json::from_slice(canonical).map_err(|_| TransportError::InvalidFrame)?;
    let request = value
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("request"))
        .and_then(Value::as_str)
        .ok_or(TransportError::InvalidFrame)?;
    if request.len() > crate::enrollment::MAX_REQUEST_BYTES * 2 {
        return Err(TransportError::MessageTooLarge);
    }
    hex::decode(request).ok_or(TransportError::InvalidFrame)
}

pub fn enrollment_ack_accepted(encoded: &[u8]) -> Result<bool, TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let value: Value = serde_json::from_slice(&encoded[..encoded.len() - 64])
        .map_err(|_| TransportError::InvalidFrame)?;
    let payload = value
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(TransportError::InvalidFrame)?;
    let accepted = payload
        .get("accepted")
        .and_then(Value::as_bool)
        .ok_or(TransportError::InvalidFrame)?;
    if accepted {
        let request = payload
            .get("request")
            .and_then(Value::as_str)
            .ok_or(TransportError::InvalidFrame)?;
        let code = payload
            .get("code")
            .and_then(Value::as_str)
            .ok_or(TransportError::InvalidFrame)?;
        if hex::decode(request).is_none() || hex::decode(code).is_none() {
            return Err(TransportError::InvalidFrame);
        }
    } else if payload.len() != 1 {
        return Err(TransportError::InvalidFrame);
    }
    Ok(accepted)
}

pub fn enrollment_ack_offer(encoded: &[u8]) -> Result<(Vec<u8>, Vec<u8>), TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let value: Value = serde_json::from_slice(&encoded[..encoded.len() - 64])
        .map_err(|_| TransportError::InvalidFrame)?;
    let payload = value
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(TransportError::InvalidFrame)?;
    if payload.get("accepted").and_then(Value::as_bool) != Some(true) || payload.len() != 3 {
        return Err(TransportError::InvalidFrame);
    }
    let request = payload
        .get("request")
        .and_then(Value::as_str)
        .and_then(hex::decode)
        .ok_or(TransportError::InvalidFrame)?;
    let code = payload
        .get("code")
        .and_then(Value::as_str)
        .and_then(hex::decode)
        .ok_or(TransportError::InvalidFrame)?;
    if request.len() > crate::enrollment::MAX_REQUEST_BYTES
        || code.len() != crate::enrollment::CODE_BYTES
    {
        return Err(TransportError::InvalidFrame);
    }
    Ok((request, code))
}
