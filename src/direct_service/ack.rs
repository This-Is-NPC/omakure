use crate::direct_transport::{envelope_kind_hint, envelope_nonce, envelope_view, verify_envelope};
use serde_json::Value;

pub(super) struct VerifiedAck {
    pub(super) accepted: Option<bool>,
    pub(super) error_code: Option<u16>,
}

pub(super) fn verified_ack(
    body: &[u8],
    peer_node_id: &str,
    peer_identity_key: &[u8; 32],
    session_id: &[u8; 32],
    kind: &str,
    id_field: &str,
    expected_id: &str,
) -> Option<VerifiedAck> {
    if envelope_kind_hint(body) != Some(kind) {
        return None;
    }
    let nonce = envelope_nonce(body).ok()?;
    verify_envelope(
        body,
        peer_node_id,
        peer_identity_key,
        kind,
        session_id,
        &nonce,
    )
    .ok()?;
    let view = envelope_view(body).ok()?;
    let ack = view.payload.as_object()?;
    if ack.get(id_field).and_then(Value::as_str) != Some(expected_id) {
        return None;
    }
    Some(VerifiedAck {
        accepted: ack.get("accepted").and_then(Value::as_bool),
        error_code: ack
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_u64)
            .and_then(|code| u16::try_from(code).ok()),
    })
}
