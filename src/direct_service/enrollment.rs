use super::error::DirectServiceError;
use super::stream::{initiator_deadline, read_frame, set_stream_timeouts, time_until, write_bytes};
use crate::direct_transport::{
    ENVELOPE_KIND, HandshakeRole, TransportError, TransportSession, enrollment_ack_accepted,
    enrollment_ack_offer, enrollment_request_bytes, envelope_nonce, sign_manual_ack,
    sign_manual_request, unix_seconds, verify_envelope,
};
use crate::enrollment::{EnrollmentRole, ManualEnrollmentRequest};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::node_transport::LocalTransport;
use crate::util::entropy;
use std::net::{SocketAddr, TcpStream};
use std::time::Instant;

type ManualEnrollmentReciprocal = Option<(Vec<u8>, Vec<u8>)>;

pub fn request_manual_enrollment(
    endpoint: SocketAddr,
    context: &NodeContext,
    request: &[u8],
) -> Result<ManualEnrollmentReciprocal, DirectServiceError> {
    if request.len() > crate::enrollment::MAX_REQUEST_BYTES {
        return Err(DirectServiceError::Protocol(
            TransportError::MessageTooLarge,
        ));
    }
    let local_request = ManualEnrollmentRequest::decode(request)
        .map_err(|_| DirectServiceError::Protocol(TransportError::InvalidFrame))?;
    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let deadline = initiator_deadline(Instant::now());
    let mut stream = TcpStream::connect_timeout(&endpoint, time_until(deadline)?)?;
    set_stream_timeouts(&stream, deadline).map_err(|_| TransportError::Internal)?;
    let mut handshake = local.handshake(HandshakeRole::Initiator)?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    handshake.read_next(&read_frame(&mut stream, deadline)?, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() == identity.public_status().node_id {
        return Err(TransportError::IdentityMismatch.into());
    }
    let mut session = handshake.into_session()?;
    let mut nonce = [0u8; 16];
    entropy::fill_bytes(&mut nonce);
    let message = sign_manual_request(
        &identity,
        session.session_id(),
        nonce,
        request,
        unix_seconds(),
    )?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &message.encoded())?,
        deadline,
    )?;
    let response = session.read(&read_frame(&mut stream, deadline)?)?;
    if response.kind != ENVELOPE_KIND {
        return Err(TransportError::NotEnrolled.into());
    }
    verify_envelope(
        &response.body,
        remote.node_id(),
        remote.identity_key(),
        "manual_ack",
        session.session_id(),
        &nonce,
    )?;
    let accepted = enrollment_ack_accepted(&response.body)?;
    let reciprocal = if accepted {
        Some(enrollment_ack_offer(&response.body)?)
    } else {
        None
    };
    registry.record_transport_audit(
        "enrollment_request",
        remote.node_id(),
        Some(session.session_id()),
        Some(0),
        request.len() + response.body.len(),
        if accepted { "accepted" } else { "rejected" },
        None,
    )?;
    if let Some((reciprocal_request, reciprocal_code)) = reciprocal {
        let reciprocal_request = ManualEnrollmentRequest::decode(&reciprocal_request)
            .map_err(|_| TransportError::InvalidFrame)?;
        reciprocal_request
            .verify(unix_seconds())
            .map_err(|_| TransportError::InvalidFrame)?;
        if reciprocal_request.proposer_node_id != remote.node_id()
            || reciprocal_request.proposer_xonly != *remote.identity_key()
            || reciprocal_request.proposer_transport_x25519 != *remote.transport_public()
            || reciprocal_request.pairing_id != local_request.pairing_id
        {
            return Err(TransportError::IdentityMismatch.into());
        }
        registry.stage_manual_enrollment(
            &reciprocal_request,
            remote.as_bytes(),
            "local-enrollment-request",
            "staged reciprocal manual enrollment request",
            unix_seconds(),
        )?;
        Ok(Some((reciprocal_request.encode(), reciprocal_code)))
    } else {
        Ok(None)
    }
}

pub(super) fn serve_enrollment_request(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    identity: &NodeIdentity,
    registry: &NodeRegistry,
    remote: &crate::direct_transport::TransportCertificate,
    context: &NodeContext,
    deadline: Instant,
) -> Result<(), DirectServiceError> {
    let frame = read_frame(stream, deadline)?;
    let request = session.read(&frame)?;
    if request.kind != ENVELOPE_KIND {
        return Err(TransportError::NotEnrolled.into());
    }
    let nonce = envelope_nonce(&request.body)?;
    verify_envelope(
        &request.body,
        remote.node_id(),
        remote.identity_key(),
        "manual_request",
        session.session_id(),
        &nonce,
    )?;
    let request_bytes = enrollment_request_bytes(&request.body)?;
    let enrollment_request = ManualEnrollmentRequest::decode(&request_bytes)
        .map_err(|_| TransportError::InvalidFrame)?;
    enrollment_request
        .verify(unix_seconds())
        .map_err(|error| match error {
            crate::enrollment::EnrollmentError::Expired => TransportError::Expired,
            crate::enrollment::EnrollmentError::IdentityMismatch => {
                TransportError::IdentityMismatch
            }
            _ => TransportError::InvalidFrame,
        })?;
    if enrollment_request.proposer_node_id != remote.node_id()
        || enrollment_request.proposer_xonly != *remote.identity_key()
        || enrollment_request.proposer_transport_x25519 != *remote.transport_public()
    {
        return Err(TransportError::IdentityMismatch.into());
    }
    crate::operations::node::manual_enrollment_enabled(context)
        .map_err(|_| TransportError::NotEnrolled)?;
    registry.stage_manual_enrollment(
        &enrollment_request,
        remote.as_bytes(),
        "authenticated-untrusted",
        "authenticated manual enrollment request",
        unix_seconds(),
    )?;
    let transport =
        LocalTransport::load_existing(context, identity).map_err(|_| TransportError::Internal)?;
    let reciprocal = ManualEnrollmentRequest::create_with_pairing_id(
        identity,
        *transport.certificate().transport_public(),
        EnrollmentRole::Conductor,
        Vec::new(),
        unix_seconds(),
        300,
        enrollment_request.pairing_id,
    )
    .map_err(|_| TransportError::InvalidFrame)?;
    let reciprocal_request = reciprocal.request.encode();
    let ack = sign_manual_ack(
        identity,
        session.session_id(),
        nonce,
        true,
        Some(&reciprocal_request),
        Some(&reciprocal.code),
        unix_seconds(),
    )?;
    write_bytes(
        stream,
        &session.write(ENVELOPE_KIND, &ack.encoded())?,
        deadline,
    )?;
    Ok(())
}
