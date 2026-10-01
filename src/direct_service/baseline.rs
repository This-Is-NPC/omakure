use super::ack::verified_ack;
use super::connection::ConnectionState;
use super::error::DirectServiceError;
use super::outbox::dispatch_answer_deadline;
use crate::direct_transport::{TransportError, unix_seconds};
use crate::node_identity::NodeIdentity;
use crate::util::entropy;
use crate::util::hex;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A baseline handed to the session thread, with the channel its answer goes
/// back on.
pub(super) struct PendingBaseline {
    /// The already-signed manifest bytes. The service never signs a manifest:
    /// the publisher key does that, in whatever process holds it, and this
    /// thread only carries what it is given.
    pub(super) manifest: Vec<u8>,
    /// Script bodies in manifest order. No paths travel with them -- the
    /// manifest is the only thing that says where a script goes.
    pub(super) bodies: Vec<Vec<u8>>,
    pub(super) baseline_id: String,
    pub(super) deadline: Instant,
    pub(super) reply: std::sync::mpsc::SyncSender<BaselinePushOutcome>,
}

/// What one baseline push came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselinePushOutcome {
    pub baseline_id: String,
    /// Whether the peer said anything at all. A refusal on trust, role, or
    /// capability is silent by design, so `false` is a real answer and not a
    /// transport failure.
    pub answered: bool,
    pub accepted: bool,
    pub code: u16,
}

/// Pushes baselines over the sessions the running service already holds.
///
/// Its own type rather than another method on `CueDispatcher`, because the two
/// grant different powers: one asks a Performer to run code it already has,
/// the other supplies the code. A handle that did both would be one thing to
/// pass around when the whole design keeps them apart.
#[derive(Clone)]
pub struct BaselineDispatcher {
    pub(super) state: Arc<ConnectionState>,
}

impl BaselineDispatcher {
    /// Push one already-signed baseline over the session this service holds.
    ///
    /// The same constraint the Cue path is built around, for the same reason: a
    /// node holds one session per peer, so a separate process cannot reach a
    /// peer the running service is already connected to. Baseline delivery does
    /// not get its own way in — it uses the outbox, and the thread that owns
    /// the session does the writing.
    ///
    /// This service never signs the manifest. The publisher key is held apart
    /// from everything this process touches, and a signing path here would put
    /// "can order a run" and "can author what runs" back in one place, which is
    /// the separation `node_registry` refuses in the other direction.
    ///
    /// `NotEnrolled` means there is no live session with that peer, which is a
    /// different fact from a refusal and is reported as one.
    pub fn push_baseline(
        &self,
        peer_node_id: &str,
        manifest: &[u8],
        bodies: &[Vec<u8>],
        wait: Duration,
    ) -> Result<BaselinePushOutcome, DirectServiceError> {
        // The same hole the Cue path had, and it matters more here: this is the
        // path that supplies the code, so an ungated push would keep shipping
        // executable content to a machine the fleet has just disowned.
        self.state.require_active_peer(peer_node_id)?;
        let parsed = crate::baseline::SignedBaselineManifest::decode(manifest)
            .map_err(|_| TransportError::InvalidFrame)?;
        // Named from the manifest rather than taken from the caller, so the
        // reply this outcome is matched against can only be an answer about the
        // set that was actually sent.
        let baseline_id = parsed
            .baseline_id()
            .map(|id| hex::encode(&id))
            .map_err(|_| TransportError::InvalidFrame)?;
        if bodies.len() != parsed.entries.len() {
            return Err(TransportError::InvalidFrame.into());
        }
        let (reply, answers) = std::sync::mpsc::sync_channel(1);
        self.state.enqueue_baseline(
            peer_node_id,
            PendingBaseline {
                manifest: manifest.to_vec(),
                bodies: bodies.to_vec(),
                baseline_id: String::clone(&baseline_id),
                deadline: Instant::now() + wait,
                reply,
            },
        )?;
        // A little past the session thread's own deadline, so the answer it is
        // about to send wins over this timeout.
        match answers.recv_timeout(dispatch_answer_deadline(wait)) {
            Ok(outcome) => Ok(outcome),
            // The session ended, or it never got to us. Neither is a verdict.
            Err(_) => Ok(BaselinePushOutcome {
                baseline_id,
                answered: false,
                accepted: false,
                code: 0,
            }),
        }
    }

    /// Whether a live session with this peer exists to carry a baseline.
    pub fn has_session(&self, peer_node_id: &str) -> bool {
        self.state.holds_session(peer_node_id)
    }
}

impl PendingBaseline {
    /// Answer the waiting caller. A closed channel means it gave up; that is
    /// not an error here, and must not take the session down with it.
    pub(super) fn answer(self, answered: bool, accepted: bool, code: u16) {
        let _ = self.reply.try_send(BaselinePushOutcome {
            baseline_id: self.baseline_id,
            answered,
            accepted,
            code,
        });
    }
}

/// What an inbound envelope turned out to be for the baseline this session sent.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BaselineAckMatch {
    /// Not the ack for this baseline; the dispatcher keeps looking.
    Other,
    /// The ack arrived inside the budget and the caller has been answered.
    Answered,
    /// The ack arrived after the budget ran out. The caller was already told
    /// `answered: false` and cannot be told anything else, so this is the only
    /// place the true outcome can be recorded.
    Late { accepted: bool, code: u16 },
}

/// One baseline written on this session, waiting for its ack.
///
/// Simpler than `OutboundCue` because it is finished at the ack: a Cue's real
/// answer is the outcome of a run that has not started yet, while a baseline
/// either installed or did not by the time the peer replies.
pub(super) struct OutboundBaseline {
    pending: PendingBaseline,
    /// Kept beside `pending`, because answering the caller moves the real
    /// `PendingBaseline` out and leaves a placeholder with an empty id. The
    /// correlation has to outlive the answer or a late ack has nothing to
    /// match against.
    baseline_id: String,
    /// Set once the caller has been answered, by the ack or by the budget.
    answered: bool,
}

impl OutboundBaseline {
    pub(super) fn new(pending: PendingBaseline) -> Self {
        Self {
            baseline_id: pending.baseline_id.clone(),
            pending,
            answered: false,
        }
    }

    /// Whether this slot still bars the next baseline from going out.
    ///
    /// A slot whose caller has been answered is kept only for correlation, so
    /// it must not hold the queue: the "one in flight per session" bound is
    /// about unanswered bytes on the wire, not about remembering an id.
    pub(super) fn is_answered(&self) -> bool {
        self.answered
    }

    /// Take the `baseline_ack` for this baseline out of the stream, if this is
    /// it.
    pub(super) fn absorb_ack(
        &mut self,
        body: &[u8],
        peer_node_id: &str,
        peer_identity_key: &[u8; 32],
        session_id: &[u8; 32],
    ) -> BaselineAckMatch {
        let Some(ack) = verified_ack(
            body,
            peer_node_id,
            peer_identity_key,
            session_id,
            crate::baseline_push::KIND_ACK,
            "baseline_id",
            &self.baseline_id,
        ) else {
            return BaselineAckMatch::Other;
        };
        let accepted = ack.accepted;
        let code = if accepted {
            0
        } else {
            ack.error_code
                .unwrap_or_else(|| crate::baseline_push::BaselineCode::InvalidMessage.code())
        };
        if self.answered {
            return BaselineAckMatch::Late { accepted, code };
        }
        self.answered = true;
        std::mem::replace(&mut self.pending, placeholder_baseline()).answer(true, accepted, code);
        BaselineAckMatch::Answered
    }

    /// Stop the caller waiting once the budget runs out.
    ///
    /// Silence is a real answer here: a receiver that refused on trust, role,
    /// or capability says nothing by design, so `answered = false` is reported
    /// rather than retried.
    ///
    /// The slot itself is kept. The budget bounds how long the *caller* waits,
    /// and nothing about it says the Performer will stay quiet: an ack that
    /// arrives a moment later is still this node's own answer to its own push,
    /// and a session that had forgotten the id would hand it to the receive
    /// half, which judges `baseline_push` messages and can only call a
    /// `baseline_ack` malformed.
    pub(super) fn expire_if_due(&mut self) {
        if self.answered || Instant::now() < self.pending.deadline {
            return;
        }
        self.answered = true;
        std::mem::replace(&mut self.pending, placeholder_baseline()).answer(false, false, 0);
    }
}

/// A spent `PendingBaseline`, so the real one can be moved out to answer with.
///
/// Its channel has no receiver, so answering it is a no-op by construction.
fn placeholder_baseline() -> PendingBaseline {
    let (reply, _) = std::sync::mpsc::sync_channel(1);
    PendingBaseline {
        manifest: Vec::new(),
        bodies: Vec::new(),
        baseline_id: String::new(),
        deadline: Instant::now(),
        reply,
    }
}

/// Sign the `baseline_push` for a queued baseline on this session.
///
/// The size bound is applied here, on the sending side, so an oversized
/// baseline is refused before any of it goes on the wire rather than after the
/// receiver has read a megabyte of it.
pub(super) fn sign_pending_baseline(
    identity: &NodeIdentity,
    session_id: &[u8; 32],
    pending: &PendingBaseline,
) -> Result<Vec<u8>, u16> {
    let total: usize = pending.bodies.iter().map(Vec::len).sum();
    if total > crate::baseline_push::MAX_PUSH_SCRIPT_BYTES {
        return Err(crate::baseline_push::BaselineCode::TooLarge.code());
    }
    let now = unix_seconds();
    let mut nonce = [0u8; 16];
    entropy::fill_bytes(&mut nonce);
    crate::direct_transport::sign_baseline_envelope(
        identity,
        crate::baseline_push::KIND_PUSH,
        session_id,
        nonce,
        crate::baseline_push::BaselinePush::encode(&pending.manifest, &pending.bodies),
        now,
    )
    .map(|envelope| envelope.encoded())
    .map_err(|_| crate::baseline_push::BaselineCode::InvalidMessage.code())
}
