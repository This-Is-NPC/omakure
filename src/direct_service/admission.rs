use super::{
    ADMISSION_BYTES, DIRECT_MAX_BYTES, DIRECT_MAX_HANDSHAKES, DIRECT_MAX_NODE_BYTES,
    DIRECT_MAX_NODE_HANDSHAKES, DIRECT_MAX_NODE_SESSIONS, DIRECT_MAX_SESSIONS,
    DIRECT_MAX_SOURCE_BYTES, DIRECT_MAX_SOURCE_ENTRIES, DIRECT_MAX_SOURCE_HANDSHAKES,
    DIRECT_MAX_SOURCE_SESSIONS, DIRECT_RATE_LIMIT, DIRECT_RATE_WINDOW,
};
use crate::direct_transport::TransportError;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
pub(super) struct AdmissionState {
    pub(super) handshakes: usize,
    sessions: usize,
    pub(super) bytes: usize,
    pub(super) sources: HashMap<IpAddr, SourceAdmission>,
    nodes: HashMap<String, SourceAdmission>,
}

#[derive(Default, Clone)]
pub(super) struct SourceAdmission {
    handshakes: usize,
    sessions: usize,
    pub(super) bytes: usize,
    attempts: VecDeque<Instant>,
}

pub(super) struct AdmissionController {
    pub(super) state: Mutex<AdmissionState>,
}

pub(super) struct AdmissionReservation {
    admission: Arc<AdmissionController>,
    /// The per-source-IP rate key, or `None` for a dial this node made.
    ///
    /// Only inbound work has a source to budget. A dial of our own has no
    /// stranger behind it to hold to account.
    source: Option<IpAddr>,
    node_id: Option<String>,
    phase: AdmissionPhase,
    bytes: usize,
}

#[derive(Clone, Copy)]
enum AdmissionPhase {
    Handshake,
    Session,
}

impl AdmissionController {
    pub(super) fn reserve(
        self: &Arc<Self>,
        source: IpAddr,
        now: Instant,
    ) -> Option<AdmissionReservation> {
        let mut state = self.state.lock().ok()?;
        prune_sources(&mut state, now);
        if !state.sources.contains_key(&source) {
            if state.sources.len() >= DIRECT_MAX_SOURCE_ENTRIES {
                return None;
            }
            state.sources.insert(source, SourceAdmission::default());
        }
        let (source_handshakes, source_bytes, source_attempts) = {
            let source_state = state.sources.get_mut(&source)?;
            while source_state
                .attempts
                .front()
                .is_some_and(|attempt| now.duration_since(*attempt) >= DIRECT_RATE_WINDOW)
            {
                source_state.attempts.pop_front();
            }
            (
                source_state.handshakes,
                source_state.bytes,
                source_state.attempts.len(),
            )
        };
        let source_allowed = state.handshakes < DIRECT_MAX_HANDSHAKES
            && state.bytes.saturating_add(ADMISSION_BYTES) <= DIRECT_MAX_BYTES
            && source_handshakes < DIRECT_MAX_SOURCE_HANDSHAKES
            && source_bytes.saturating_add(ADMISSION_BYTES) <= DIRECT_MAX_SOURCE_BYTES
            && source_attempts < DIRECT_RATE_LIMIT;
        if !source_allowed {
            return None;
        }
        state.handshakes += 1;
        state.bytes += ADMISSION_BYTES;
        let source_state = state.sources.get_mut(&source)?;
        source_state.attempts.push_back(now);
        source_state.handshakes += 1;
        source_state.bytes += ADMISSION_BYTES;
        Some(AdmissionReservation {
            admission: Arc::clone(self),
            source: Some(source),
            node_id: None,
            phase: AdmissionPhase::Handshake,
            bytes: ADMISSION_BYTES,
        })
    }

    /// Take global admission capacity for a dial this node is making.
    ///
    /// Deliberately takes no per-source-IP budget. That budget bounds
    /// unauthenticated pressure arriving from a stranger, and an outgoing dial
    /// to a configured static peer is neither. Charging it to the local
    /// address made every node that shares an address with its peers --
    /// loopback, host networking, several nodes in one container -- spend
    /// their inbound flood budget on its own outgoing links. The global
    /// handshake, byte, and session ceilings still apply, and the peer's
    /// certificate is still charged per identity once it authenticates.
    pub(super) fn reserve_dial(self: &Arc<Self>) -> Option<AdmissionReservation> {
        let mut state = self.state.lock().ok()?;
        if state.handshakes >= DIRECT_MAX_HANDSHAKES
            || state.bytes.saturating_add(ADMISSION_BYTES) > DIRECT_MAX_BYTES
        {
            return None;
        }
        state.handshakes += 1;
        state.bytes += ADMISSION_BYTES;
        Some(AdmissionReservation {
            admission: Arc::clone(self),
            source: None,
            node_id: None,
            phase: AdmissionPhase::Handshake,
            bytes: ADMISSION_BYTES,
        })
    }

    pub(super) fn migrate_node(
        &self,
        reservation: &mut AdmissionReservation,
        node_id: &str,
    ) -> Result<(), TransportError> {
        // Keep the pre-auth IP reservation and add an authenticated node
        // reservation. This enforces both dimensions without allowing a node
        // to bypass limits by changing source addresses.
        let mut state = self.state.lock().map_err(|_| TransportError::Internal)?;
        if reservation.node_id.as_deref() == Some(node_id) {
            return Ok(());
        }
        if reservation
            .source
            .is_some_and(|source| !state.sources.contains_key(&source))
        {
            return Err(TransportError::Internal);
        }
        let current = SourceAdmission {
            handshakes: usize::from(matches!(reservation.phase, AdmissionPhase::Handshake)),
            sessions: usize::from(matches!(reservation.phase, AdmissionPhase::Session)),
            bytes: reservation.bytes,
            attempts: VecDeque::new(),
        };
        let existing = state.nodes.get(node_id).cloned().unwrap_or_default();
        if existing.handshakes + current.handshakes > DIRECT_MAX_NODE_HANDSHAKES
            || existing.sessions + current.sessions > DIRECT_MAX_NODE_SESSIONS
            || existing.bytes.saturating_add(current.bytes) > DIRECT_MAX_NODE_BYTES
        {
            return Err(TransportError::RateLimited);
        }
        state.nodes.insert(
            node_id.to_string(),
            SourceAdmission {
                handshakes: existing.handshakes + current.handshakes,
                sessions: existing.sessions + current.sessions,
                bytes: existing.bytes + current.bytes,
                attempts: existing
                    .attempts
                    .into_iter()
                    .chain(current.attempts)
                    .collect(),
            },
        );
        reservation.node_id = Some(node_id.to_string());
        Ok(())
    }
}

fn prune_sources(state: &mut AdmissionState, now: Instant) {
    for source in state.sources.values_mut() {
        while source
            .attempts
            .front()
            .is_some_and(|attempt| now.duration_since(*attempt) >= DIRECT_RATE_WINDOW)
        {
            source.attempts.pop_front();
        }
    }
    state.sources.retain(|_, source| {
        source.handshakes != 0
            || source.sessions != 0
            || source.bytes != 0
            || !source.attempts.is_empty()
    });
}

impl AdmissionReservation {
    pub(super) fn promote_session(&mut self) -> Result<(), TransportError> {
        let mut state = self
            .admission
            .state
            .lock()
            .map_err(|_| TransportError::Internal)?;
        let source_sessions = self
            .source
            .map(|source| {
                state
                    .sources
                    .get(&source)
                    .ok_or(TransportError::Internal)
                    .map(|source| source.sessions)
            })
            .transpose()?;
        let node_sessions = self
            .node_id
            .as_deref()
            .map(|node_id| {
                state
                    .nodes
                    .get(node_id)
                    .ok_or(TransportError::Internal)
                    .map(|node| node.sessions)
            })
            .transpose()?;
        if state.sessions >= DIRECT_MAX_SESSIONS
            || source_sessions.is_some_and(|sessions| sessions >= DIRECT_MAX_SOURCE_SESSIONS)
            || node_sessions.is_some_and(|sessions| sessions >= DIRECT_MAX_NODE_SESSIONS)
        {
            return Err(TransportError::RateLimited);
        }
        state.handshakes = state.handshakes.saturating_sub(1);
        state.sessions += 1;
        if let Some(source) = self.source {
            let source_state = state
                .sources
                .get_mut(&source)
                .ok_or(TransportError::Internal)?;
            source_state.handshakes = source_state.handshakes.saturating_sub(1);
            source_state.sessions += 1;
        }
        if let Some(node_id) = self.node_id.as_deref() {
            let node_state = state
                .nodes
                .get_mut(node_id)
                .expect("node admission exists after preflight");
            node_state.handshakes = node_state.handshakes.saturating_sub(1);
            node_state.sessions += 1;
        }
        self.phase = AdmissionPhase::Session;
        Ok(())
    }
}

impl Drop for AdmissionReservation {
    fn drop(&mut self) {
        let Ok(mut state) = self.admission.state.lock() else {
            return;
        };
        match self.phase {
            AdmissionPhase::Handshake => {
                state.handshakes = state.handshakes.saturating_sub(1);
            }
            AdmissionPhase::Session => {
                state.sessions = state.sessions.saturating_sub(1);
            }
        }
        state.bytes = state.bytes.saturating_sub(self.bytes);
        state.release_source(self.source, self.phase, self.bytes);
        state.release_node(self.node_id.as_deref(), self.phase, self.bytes);
    }
}

impl SourceAdmission {
    fn release(&mut self, phase: AdmissionPhase, bytes: usize) {
        match phase {
            AdmissionPhase::Handshake => self.handshakes = self.handshakes.saturating_sub(1),
            AdmissionPhase::Session => self.sessions = self.sessions.saturating_sub(1),
        }
        self.bytes = self.bytes.saturating_sub(bytes);
    }

    fn idle(&self) -> bool {
        self.handshakes == 0 && self.sessions == 0 && self.bytes == 0
    }
}

impl AdmissionState {
    fn release_source(&mut self, source: Option<IpAddr>, phase: AdmissionPhase, bytes: usize) {
        let Some(source) = source else { return };
        let Some(bucket) = self.sources.get_mut(&source) else {
            return;
        };
        bucket.release(phase, bytes);
        // Recent arrivals keep their rate-limit history even when idle.
        if bucket.idle() && bucket.attempts.is_empty() {
            self.sources.remove(&source);
        }
    }

    fn release_node(&mut self, node_id: Option<&str>, phase: AdmissionPhase, bytes: usize) {
        let Some(node_id) = node_id else { return };
        let Some(bucket) = self.nodes.get_mut(node_id) else {
            return;
        };
        bucket.release(phase, bytes);
        if bucket.idle() {
            self.nodes.remove(node_id);
        }
    }
}
