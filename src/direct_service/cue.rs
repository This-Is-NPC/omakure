use super::IDLE_TIMEOUT;
use super::ack::verified_ack;
use super::connection::ConnectionState;
use super::error::DirectServiceError;
use super::outbox::dispatch_answer_deadline;
use super::session::{Readiness, peer_authorization, wait_readable};
use super::stream::{initiator_deadline, read_frame, set_stream_timeouts, time_until, write_bytes};
use crate::direct_health::HealthSession;
use crate::direct_transport::{
    ENVELOPE_KIND, HandshakeRole, TransportError, TransportSession, authorize_peer, envelope_nonce,
    sign_probe, unix_seconds, verify_envelope,
};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::node_transport::LocalTransport;
use crate::remote_cue::CueCode;
use crate::util::entropy;
use crate::util::hex;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A Cue handed to the session thread, with the channel its answer goes back on.
pub(super) struct PendingCue {
    pub(super) cue_id: String,
    pub(super) script: String,
    pub(super) reason: String,
    pub(super) expected_run_id: String,
    pub(super) deadline: Instant,
    pub(super) reply: std::sync::mpsc::SyncSender<CueDispatchOutcome>,
}

/// Sends Cues over the sessions the running service already holds.
///
/// This is the path that works in a managed fleet. A separate process cannot
/// dial a peer this node is already connected to -- `register` refuses it, and
/// should, because two sessions with one peer would give the Health Plane two
/// cursors for the same node. So the instruction is handed to the thread that
/// owns the session instead of racing it for a new one.
#[derive(Clone)]
pub struct CueDispatcher {
    pub(super) state: Arc<ConnectionState>,
}

impl CueDispatcher {
    /// Send one Cue and wait for as much of an answer as arrives in budget.
    ///
    /// `NotEnrolled` means there is no live session with that peer, which is a
    /// different fact from a refusal and is reported as one.
    pub fn dispatch(
        &self,
        peer_node_id: &str,
        script: &str,
        reason: &str,
        wait: Duration,
        cue_id: Option<&str>,
    ) -> Result<CueDispatchOutcome, DirectServiceError> {
        if !crate::remote_cue::is_well_formed_script_name(script) {
            return Err(TransportError::InvalidFrame.into());
        }
        if reason.is_empty() || reason.len() > crate::remote_cue::MAX_REASON_BYTES {
            return Err(TransportError::InvalidFrame.into());
        }
        // Before a cue id is resolved, so a refused instruction leaves no id an
        // operator could mistake for one that was sent.
        self.state.require_active_peer(peer_node_id)?;
        let cue_id = resolve_cue_id(cue_id)?;
        let expected_run_id =
            crate::health_plane::report::opaque_run_id(&crate::remote_cue::derive_run_id(&cue_id));
        let (reply, answers) = std::sync::mpsc::sync_channel(1);
        self.state.enqueue_cue(
            peer_node_id,
            PendingCue {
                cue_id: cue_id.clone(),
                script: script.to_string(),
                reason: reason.to_string(),
                expected_run_id: expected_run_id.clone(),
                deadline: Instant::now() + wait,
                reply,
            },
        )?;
        // A little past the session thread's own deadline, so the answer it is
        // about to send wins over this timeout.
        match answers.recv_timeout(dispatch_answer_deadline(wait)) {
            Ok(outcome) => Ok(outcome),
            // The session ended, or it never got to us. Neither is a verdict.
            Err(_) => Ok(CueDispatchOutcome {
                cue_id,
                expected_run_id,
                answered: false,
                accepted: false,
                code: 0,
                outcome_seen: false,
            }),
        }
    }

    /// Whether a live session with this peer exists to carry a Cue.
    pub fn has_session(&self, peer_node_id: &str) -> bool {
        self.state.holds_session(peer_node_id)
    }
}

impl PendingCue {
    /// Answer the waiting caller. A closed channel means it gave up; that is
    /// not an error here, and must not take the session down with it.
    pub(super) fn answer(self, answered: bool, accepted: bool, code: u16, outcome_seen: bool) {
        let _ = self.reply.try_send(CueDispatchOutcome {
            cue_id: self.cue_id,
            expected_run_id: self.expected_run_id,
            answered,
            accepted,
            code,
            outcome_seen,
        });
    }
}

/// One Cue written on this session, waiting for its ack and then its outcome.
pub(super) struct OutboundCue {
    pending: PendingCue,
    /// `None` until the Performer answers. A refusal on trust, role, or
    /// capability is silent by design, so staying `None` is a real answer.
    code: Option<u16>,
}

impl OutboundCue {
    pub(super) fn new(pending: PendingCue) -> Self {
        Self {
            pending,
            code: None,
        }
    }

    /// Take the `cue_ack` for this Cue out of the stream, if this is it.
    ///
    /// A refusal answers the caller here; an acceptance keeps waiting for the
    /// outcome, which arrives as an ordinary Signal the Health Plane records.
    /// Both are still *this Cue's ack*, and saying so is the whole point: an
    /// acceptance that reported "not mine" was handed on to the receive half,
    /// which judges `cue_dispatch` messages and can only read a `cue_ack` as a
    /// malformed one -- so every accepted Cue wrote `cue_rejected` /
    /// `invalid_message` into the Conductor's own audit table.
    pub(super) fn absorb_ack(
        &mut self,
        body: &[u8],
        peer_node_id: &str,
        peer_identity_key: &[u8; 32],
        session_id: &[u8; 32],
    ) -> CueAckMatch {
        let Some(ack) = verified_ack(
            body,
            peer_node_id,
            peer_identity_key,
            session_id,
            crate::remote_cue::KIND_ACK,
            "cue_id",
            &self.pending.cue_id,
        ) else {
            return CueAckMatch::Other;
        };
        let accepted = ack.accepted;
        if accepted {
            self.code = Some(0);
            return CueAckMatch::Accepted;
        }
        let code = ack
            .error_code
            .unwrap_or_else(|| CueCode::InvalidMessage.code());
        std::mem::replace(&mut self.pending, placeholder_cue()).answer(true, false, code, false);
        CueAckMatch::Refused
    }

    /// Finish once the outcome is recorded or the budget runs out.
    ///
    /// The stop condition is read back from the registry after the Health Plane
    /// session verified and recorded the Signal, never from a payload this code
    /// inspected itself.
    pub(super) fn resolve(&mut self, registry: &NodeRegistry, peer_node_id: &str) -> bool {
        if self.code == Some(0) {
            let plane = crate::health_plane::HealthPlane::new(registry);
            if signal_recorded(&plane, peer_node_id, &self.pending.expected_run_id) {
                std::mem::replace(&mut self.pending, placeholder_cue()).answer(true, true, 0, true);
                return true;
            }
        }
        if Instant::now() < self.pending.deadline {
            return false;
        }
        let answered = self.code.is_some();
        let accepted = self.code == Some(0);
        let code = self.code.unwrap_or(0);
        std::mem::replace(&mut self.pending, placeholder_cue())
            .answer(answered, accepted, code, false);
        true
    }
}

/// What an inbound envelope turned out to be for the Cue this session sent.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CueAckMatch {
    /// Not the ack for this Cue; the dispatcher keeps looking.
    Other,
    /// This Cue's ack, carrying an acceptance. The exchange is *not* over --
    /// the outcome still arrives as an ordinary Signal -- but the envelope
    /// belongs to this slot and has been taken out of the stream.
    Accepted,
    /// This Cue's ack, carrying a refusal. The caller has been answered.
    Refused,
}

/// A spent `PendingCue`, so the real one can be moved out to answer with.
///
/// Its channel has no receiver, so answering it is a no-op by construction.
fn placeholder_cue() -> PendingCue {
    let (reply, _) = std::sync::mpsc::sync_channel(1);
    PendingCue {
        cue_id: String::new(),
        script: String::new(),
        reason: String::new(),
        expected_run_id: String::new(),
        deadline: Instant::now(),
        reply,
    }
}

/// Sign the `cue_dispatch` for a queued Cue on this session.
pub(super) fn sign_pending_cue(
    identity: &NodeIdentity,
    session_id: &[u8; 32],
    pending: &PendingCue,
) -> Result<Vec<u8>, TransportError> {
    let now = unix_seconds();
    let mut nonce = [0u8; 16];
    entropy::fill_bytes(&mut nonce);
    Ok(crate::direct_transport::sign_cue_envelope(
        identity,
        crate::remote_cue::KIND_DISPATCH,
        session_id,
        nonce,
        serde_json::json!({
            "version": 1,
            "cue_id": pending.cue_id,
            "script": pending.script,
            "not_before": now,
            "expires_at": now + crate::remote_cue::MAX_LIFETIME_SECONDS as u64,
            "reason": pending.reason,
        }),
        now,
    )?
    .encoded())
}

/// Ask one trusted Performer to run a script it has already declared.
///
/// A one-shot dial mirroring `probe`, deliberately with no Conductor-side
/// durable outbox: a Cue is an instruction with a short validity window, and a
/// queue of instructions that outlive their window is a way to deliver
/// surprises. If the dial fails, the operator dials again with a new id.
///
/// Returns the minted `cue_id`, from which the Conductor can compute the opaque
/// run id it will later see on the `run-completed` Signal — which is why no
/// correlation field is added to any message.
pub fn dispatch_cue(
    endpoint: SocketAddr,
    expected_node_id: &str,
    script: &str,
    reason: &str,
    wait_seconds: u32,
    context: &NodeContext,
    cue_id: Option<&str>,
) -> Result<CueDispatchOutcome, DirectServiceError> {
    if !crate::remote_cue::is_well_formed_script_name(script) {
        return Err(TransportError::InvalidFrame.into());
    }
    if reason.is_empty() || reason.len() > 128 {
        return Err(TransportError::InvalidFrame.into());
    }

    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let deadline = initiator_deadline(Instant::now());
    let mut stream = TcpStream::connect_timeout(&endpoint, time_until(deadline)?)?;
    set_stream_timeouts(&stream, deadline)?;

    let mut handshake = local.handshake(HandshakeRole::Initiator)?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    handshake.read_next(&read_frame(&mut stream, deadline)?, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() != expected_node_id {
        return Err(TransportError::IdentityMismatch.into());
    }
    let trusted = registry.transport_peer(remote.node_id(), &hex::encode(remote.identity_key()))?;
    authorize_peer(
        &remote,
        trusted.as_ref().map(peer_authorization),
        unix_seconds(),
    )?;
    let mut session = handshake.into_session()?;

    // The responder's steady-state loop is only reachable through the existing
    // probe/ack entry. A Cue is new traffic *inside* an established session,
    // not a new way to open one, so the ritual is performed unchanged rather
    // than given a second door that would need its own review.
    let mut probe_nonce = [0u8; 16];
    entropy::fill_bytes(&mut probe_nonce);
    let probe = sign_probe(&identity, session.session_id(), probe_nonce, unix_seconds())?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &probe.encoded())?,
        deadline,
    )?;
    let response = session.read(&read_frame(&mut stream, deadline)?)?;
    if let Some(stated) = crate::direct_transport::stated_error(&response) {
        return Err(stated.into());
    }
    if response.kind != ENVELOPE_KIND {
        return Err(TransportError::InvalidFrame.into());
    }
    verify_envelope(
        &response.body,
        remote.node_id(),
        remote.identity_key(),
        "ack",
        session.session_id(),
        &probe_nonce,
    )?;

    let cue_id = resolve_cue_id(cue_id)?;
    let now = unix_seconds();
    let mut nonce = [0u8; 16];
    entropy::fill_bytes(&mut nonce);
    let dispatch = crate::direct_transport::sign_cue_envelope(
        &identity,
        crate::remote_cue::KIND_DISPATCH,
        session.session_id(),
        nonce,
        serde_json::json!({
            "version": 1,
            "cue_id": cue_id,
            "script": script,
            "not_before": now,
            "expires_at": now + crate::remote_cue::MAX_LIFETIME_SECONDS as u64,
            "reason": reason,
        }),
        now,
    )?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &dispatch.encoded())?,
        deadline,
    )?;

    // A refusal the sender is not authorized to hear is silent by design, so
    // the absence of an ack is a legitimate answer and not an error. It is
    // reported as `answered: false` rather than being turned into a code the
    // Performer never sent.
    let acknowledgement = read_cue_ack(&mut stream, &mut session, &remote, &cue_id, deadline);

    registry.record_transport_audit(
        "cue_dispatched",
        remote.node_id(),
        Some(session.session_id()),
        Some(0),
        dispatch.encoded().len(),
        "accepted",
        None,
    )?;
    // The Conductor computes the opaque run id it will see on the
    // `run-completed` Signal from the cue id it just minted. No message
    // carries a correlation field; both sides derive it.
    let expected_run_id =
        crate::health_plane::report::opaque_run_id(&crate::remote_cue::derive_run_id(&cue_id));

    // Wait for the outcome on the session already open, rather than requiring a
    // standing one. A Performer that already holds a session with this
    // Conductor refuses this dial outright -- `register` will not accept from a
    // peer it owns the dial to, nor a second connection to a peer it already
    // has -- so the configuration that would deliver the Signal is exactly the
    // one in which the Cue could not be sent. The Performer already pushes
    // Health traffic down this session unprompted; this reads it.
    let outcome_seen = if wait_seconds > 0 && acknowledgement == Some(0) {
        let until = Instant::now() + Duration::from_secs(u64::from(wait_seconds));
        set_stream_timeouts(&stream, until)?;
        await_cue_outcome(
            &mut stream,
            &mut session,
            &remote,
            &identity,
            &registry,
            &expected_run_id,
            until,
        )
    } else {
        false
    };

    Ok(CueDispatchOutcome {
        expected_run_id,
        cue_id,
        answered: acknowledgement.is_some(),
        accepted: acknowledgement.is_some_and(|code| code == 0),
        code: acknowledgement.unwrap_or(0),
        outcome_seen,
    })
}

/// Read Health traffic on this session until the Cue's outcome shows up.
///
/// The dispatcher behaves as an ordinary Conductor receiver for the duration:
/// the `HealthSession` verifies, records, and acknowledges exactly as the
/// service would, so nothing here is a second, looser path into the Health
/// Plane. The stop condition is read back from the registry after the session
/// recorded it, never from an unverified payload.
#[allow(clippy::too_many_arguments)]
fn await_cue_outcome(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    remote: &crate::direct_transport::TransportCertificate,
    identity: &NodeIdentity,
    registry: &NodeRegistry,
    expected_run_id: &str,
    until: Instant,
) -> bool {
    let mut health = HealthSession::new(
        identity,
        registry,
        remote.node_id(),
        remote.identity_key(),
        *session.session_id(),
        None,
    );
    let plane = crate::health_plane::HealthPlane::new(registry);
    loop {
        if signal_recorded(&plane, remote.node_id(), expected_run_id) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        match wait_readable(stream, crate::direct_health::TICK) {
            Readiness::Readable => {}
            Readiness::Idle => continue,
            Readiness::Closed | Readiness::Failed(_) => {
                // One last look: the Signal may have landed on the frame that
                // arrived immediately before the peer hung up.
                return signal_recorded(&plane, remote.node_id(), expected_run_id);
            }
        }
        let Ok(encoded) = read_frame(stream, until) else {
            return signal_recorded(&plane, remote.node_id(), expected_run_id);
        };
        let Ok(message) = session.read(&encoded) else {
            return false;
        };
        if message.kind != ENVELOPE_KIND {
            continue;
        }
        if let crate::direct_health::HealthOutcome::Reply(reply) =
            health.handle_envelope(&message.body)
        {
            let Ok(frame) = session.write(ENVELOPE_KIND, &reply) else {
                return false;
            };
            if write_bytes(stream, &frame, Instant::now() + IDLE_TIMEOUT).is_err() {
                return false;
            }
        }
    }
}

/// Whether this peer's recorded Signals already carry the awaited run.
fn signal_recorded(
    plane: &crate::health_plane::HealthPlane<'_>,
    node_id: &str,
    expected_run_id: &str,
) -> bool {
    plane
        .signals(
            node_id,
            crate::health_plane::bounds::SIGNAL_INBOX_CAPACITY as usize,
        )
        .map(|signals| {
            signals.iter().any(|signal| {
                signal.kind == crate::health_plane::model::SignalKind::RunCompleted
                    && signal
                        .run
                        .as_ref()
                        .is_some_and(|run| run.run_id == expected_run_id)
            })
        })
        .unwrap_or(false)
}

/// What the Conductor can honestly say about one dispatch.
///
/// `answered` is separate from `accepted` on purpose: a Performer that refuses
/// on trust, role, or capability says nothing at all, so "no answer" and
/// "refused with a code" are different facts and collapsing them would invent
/// a verdict nobody sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueDispatchOutcome {
    pub cue_id: String,
    /// The `run.run_id` the matching `run-completed` Signal will carry.
    pub expected_run_id: String,
    pub answered: bool,
    pub accepted: bool,
    pub code: u16,
    /// Whether the `run-completed` Signal for this Cue arrived before the wait
    /// budget ran out. `false` is not a failure -- the run may simply still be
    /// going, and the Signal will reach a standing session later.
    pub outcome_seen: bool,
}

/// Read one `cue_ack` for this cue id, or `None` if none arrives in budget.
///
/// The session is shared with the Health Plane, whose reporter greets a trusted
/// Conductor with a Profile the moment the connection opens, so the ack is very
/// often not the first frame. Anything that is not this cue's ack is skipped:
/// a one-shot dispatcher holds no Health session and has no business answering
/// Health traffic.
///
/// Bounded twice over -- by the connection deadline and by a frame count -- so
/// a peer cannot hold the dispatcher open by talking.
fn read_cue_ack(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    remote: &crate::direct_transport::TransportCertificate,
    cue_id: &str,
    deadline: Instant,
) -> Option<u16> {
    /// Enough for a Profile, a Pulse, and a Signal to precede the ack.
    const MAX_FRAMES_BEFORE_ACK: usize = 8;

    for _ in 0..MAX_FRAMES_BEFORE_ACK {
        if Instant::now() >= deadline {
            return None;
        }
        let frame = read_frame(stream, deadline).ok()?;
        let message = session.read(&frame).ok()?;
        if message.kind != ENVELOPE_KIND {
            continue;
        }
        if crate::direct_transport::envelope_kind_hint(&message.body)
            != Some(crate::remote_cue::KIND_ACK)
        {
            continue;
        }
        let nonce = envelope_nonce(&message.body).ok()?;
        verify_envelope(
            &message.body,
            remote.node_id(),
            remote.identity_key(),
            crate::remote_cue::KIND_ACK,
            session.session_id(),
            &nonce,
        )
        .ok()?;
        let view = crate::direct_transport::envelope_view(&message.body).ok()?;
        let ack = view.payload.as_object()?;
        // An ack for a different cue id is not an answer to this dispatch.
        if ack.get("cue_id").and_then(serde_json::Value::as_str) != Some(cue_id) {
            return None;
        }
        if ack.get("accepted").and_then(serde_json::Value::as_bool)? {
            return Some(0);
        }
        let code = ack
            .get("error")?
            .get("code")
            .and_then(serde_json::Value::as_u64)?;
        // Zero is how acceptance is spelled, so a refusal must never land on it.
        return u16::try_from(code).ok().filter(|code| *code != 0);
    }
    None
}

/// Resolve a Cue id from an optional caller-supplied idempotency key.
///
/// A supplied id is the caller's idempotency key; omitting mints a new one.
/// Malformed ids are refused before any mint or dial.
pub(super) fn resolve_cue_id(cue_id: Option<&str>) -> Result<String, DirectServiceError> {
    match cue_id {
        Some(id) if crate::remote_cue::is_well_formed_cue_id(id) => Ok(id.to_string()),
        Some(_) => Err(TransportError::InvalidFrame.into()),
        None => {
            let mut cue_id_bytes = [0u8; 16];
            entropy::fill_bytes(&mut cue_id_bytes);
            Ok(hex::encode(&cue_id_bytes))
        }
    }
}
