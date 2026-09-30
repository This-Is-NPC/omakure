use super::super::{PeerState, RegistryError};
use super::apply::{advance_cursor, record_replay_key, store_profile, store_pulse, store_signal};
use super::rows::{decode_opaque_id, health_authorization_from_row, load_peer_state};
use super::types::{HealthApplyRequest, HealthPeerState};
use crate::health_plane::bounds::{
    MAX_CONDUCTORS_PER_PERFORMER, MAX_MESSAGES_PER_PEER_PER_MINUTE, MAX_PERFORMERS_PER_CONDUCTOR,
    MAX_PROFILES_PER_PEER_PER_HOUR, MAX_SIGNALS_PER_PEER_PER_MINUTE, MIN_PULSE_INTERVAL_SECONDS,
    RATE_BURST_ALLOWANCE, RATE_HOUR_WINDOW_SECONDS, RATE_MINUTE_WINDOW_SECONDS,
    REORDER_BUFFER_ENTRIES, SIGNAL_GLOBAL_INBOX_CAPACITY, SIGNAL_INBOX_CAPACITY,
};
use crate::health_plane::model::{HealthBody, HealthCode, HealthDecision, HealthKind};
use rusqlite::{params, OptionalExtension, Transaction};

pub(super) fn evaluate(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
) -> Result<HealthDecision, RegistryError> {
    let kind = request.payload.body.kind();

    // Step 7: trust, read from the local registry only.
    let authorization = transaction
        .query_row(
            "SELECT r.state, p.role, p.capabilities, p.state
             FROM remote_identities r
             LEFT JOIN trusted_peers p ON p.node_id = r.node_id
             WHERE r.node_id = ?1",
            params![request.sender],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((identity_state, role, capabilities, trust_state)) = authorization else {
        return Ok(HealthDecision::Rejected(HealthCode::Revoked));
    };
    let authorization = health_authorization_from_row(
        request.sender,
        &identity_state,
        role,
        capabilities,
        trust_state.as_deref(),
    )?;
    if authorization.state != PeerState::Active || trust_state.is_none() {
        return Ok(HealthDecision::Rejected(HealthCode::Revoked));
    }

    // Step 8: role and the single-Conductor bound.
    let stored_role = authorization.role.code();
    if stored_role != kind.required_role() {
        return Ok(HealthDecision::Rejected(HealthCode::WrongRole));
    }
    if stored_role == 1 {
        let other_conductors: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM health_peers WHERE role = 1 AND node_id <> ?1",
            params![request.sender],
            |row| row.get(0),
        )?;
        if other_conductors >= MAX_CONDUCTORS_PER_PERFORMER {
            return Ok(HealthDecision::Rejected(HealthCode::WrongRole));
        }
    }

    // Step 9: capability, read from the local registry only.
    if let Some(required) = kind.required_capability() {
        if !authorization
            .capabilities
            .iter()
            .any(|entry| entry == required)
        {
            return Ok(HealthDecision::Rejected(HealthCode::MissingCapability));
        }
    }

    // Step 10: freshness.
    if request.created_at
        > request
            .now
            .saturating_add(crate::health_plane::bounds::MAX_FUTURE_SKEW_SECONDS)
    {
        return Ok(HealthDecision::Rejected(HealthCode::Future));
    }
    if request.now.saturating_sub(request.created_at) > crate::health_plane::bounds::MAX_AGE_SECONDS
    {
        return Ok(HealthDecision::Rejected(HealthCode::Stale));
    }

    let mut state = load_peer_state(transaction, request.sender)?;

    // Step 11: rate.
    if let Some(existing) = state.as_ref() {
        if let Some(code) = rate_check(transaction, existing, kind, request.now)? {
            return Ok(HealthDecision::Rejected(code));
        }
    }

    // Step 12: replay.
    let message_id = decode_opaque_id(&request.payload.message_id)?;
    let seen: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM health_replay_keys WHERE message_id = ?1",
        params![message_id],
        |row| row.get(0),
    )?;
    if seen > 0 {
        return Ok(HealthDecision::Rejected(HealthCode::Replay));
    }

    // Step 13: ordering.
    let cursor = state.as_ref().map(|state| state.cursor).unwrap_or(0);
    let mut hold = false;
    match &request.payload.body {
        HealthBody::Profile(profile) => {
            let last = state
                .as_ref()
                .map(|state| state.last_profile_revision)
                .unwrap_or(0);
            if profile.profile_revision <= last {
                return Ok(HealthDecision::Rejected(HealthCode::Replay));
            }
        }
        HealthBody::Pulse(pulse) => {
            let last = state
                .as_ref()
                .map(|state| state.last_pulse_sequence)
                .unwrap_or(0);
            if pulse.sequence <= last {
                return Ok(HealthDecision::Rejected(HealthCode::Replay));
            }
        }
        HealthBody::Signal(signal) => {
            let signal_id = decode_opaque_id(&signal.signal_id)?;
            let duplicate: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM health_signals
                 WHERE node_id = ?1 AND (signal_id = ?2 OR sequence = ?3)",
                params![request.sender, signal_id, signal.sequence as i64],
                |row| row.get(0),
            )?;
            if signal.sequence <= cursor || duplicate > 0 {
                return Ok(HealthDecision::Rejected(HealthCode::Replay));
            }
            if signal.sequence > cursor.saturating_add(REORDER_BUFFER_ENTRIES) {
                return Ok(HealthDecision::Rejected(HealthCode::Reordered));
            }
            hold = signal.sequence != cursor + 1;
        }
        HealthBody::Ack(_) | HealthBody::Error(_) => {}
    }

    // Step 14: capacity.
    if state.is_none() {
        let tracked: i64 =
            transaction.query_row("SELECT COUNT(*) FROM health_peers", [], |row| row.get(0))?;
        if tracked >= MAX_PERFORMERS_PER_CONDUCTOR {
            return Ok(HealthDecision::Rejected(HealthCode::QueueFull));
        }
    }
    if matches!(request.payload.body, HealthBody::Signal(_)) {
        let stored: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM health_signals WHERE node_id = ?1",
            params![request.sender],
            |row| row.get(0),
        )?;
        let global: i64 =
            transaction.query_row("SELECT COUNT(*) FROM health_signals", [], |row| row.get(0))?;
        if stored >= SIGNAL_INBOX_CAPACITY || global >= SIGNAL_GLOBAL_INBOX_CAPACITY {
            return Ok(HealthDecision::Rejected(HealthCode::QueueFull));
        }
    }

    // Step 15: apply, inside this single transaction.
    if !record_replay_key(transaction, &message_id, request.sender, request.now)? {
        return Ok(HealthDecision::Rejected(HealthCode::RateLimited));
    }
    if state.is_none() {
        transaction.execute(
            "INSERT INTO health_peers
             (node_id, role, cursor, last_profile_revision, last_pulse_sequence, last_pulse_at,
              version_incompatible_at, minute_window_start, minute_messages, minute_signals,
              hour_window_start, hour_profiles, first_seen, updated_at)
             VALUES (?1, ?2, 0, 0, 0, NULL, NULL, ?3, 0, 0, ?3, 0, ?3, ?3)",
            params![request.sender, stored_role, request.now],
        )?;
        state = load_peer_state(transaction, request.sender)?;
    }
    let Some(existing) = state else {
        return Err(RegistryError::Corrupt(
            "health peer state disappeared during apply".to_string(),
        ));
    };
    count_rate(transaction, request.sender, kind, request.now)?;

    let decision = match &request.payload.body {
        HealthBody::Profile(profile) => {
            store_profile(transaction, request, profile)?;
            transaction.execute(
                "UPDATE health_peers SET last_profile_revision = ?2, updated_at = ?3
                 WHERE node_id = ?1",
                params![request.sender, profile.profile_revision as i64, request.now],
            )?;
            HealthDecision::Accepted {
                cursor: existing.cursor,
            }
        }
        HealthBody::Pulse(pulse) => {
            store_pulse(transaction, request, pulse)?;
            transaction.execute(
                "UPDATE health_peers
                 SET last_pulse_sequence = ?2, last_pulse_at = ?3, updated_at = ?3
                 WHERE node_id = ?1",
                params![request.sender, pulse.sequence as i64, request.now],
            )?;
            HealthDecision::Accepted {
                cursor: existing.cursor,
            }
        }
        HealthBody::Signal(signal) => {
            store_signal(transaction, request, signal, hold)?;
            if hold {
                HealthDecision::Held {
                    cursor: existing.cursor,
                }
            } else {
                let cursor = advance_cursor(transaction, request.sender, signal.sequence)?;
                HealthDecision::Accepted { cursor }
            }
        }
        HealthBody::Ack(ack) => {
            let acked = decode_opaque_id(&ack.acked_message_id)?;
            transaction.execute(
                "DELETE FROM health_outbox WHERE last_message_id = ?1",
                params![acked],
            )?;
            transaction.execute(
                "UPDATE health_peers SET cursor = ?2, updated_at = ?3 WHERE node_id = ?1",
                params![request.sender, ack.cursor as i64, request.now],
            )?;
            HealthDecision::Accepted { cursor: ack.cursor }
        }
        HealthBody::Error(error) => {
            if error.code == HealthCode::UnsupportedVersion.code() {
                transaction.execute(
                    "UPDATE health_peers SET version_incompatible_at = ?2, updated_at = ?2
                     WHERE node_id = ?1",
                    params![request.sender, request.now],
                )?;
            }
            HealthDecision::Accepted {
                cursor: existing.cursor,
            }
        }
    };
    Ok(decision)
}

fn rate_check(
    transaction: &Transaction<'_>,
    state: &HealthPeerState,
    kind: HealthKind,
    now: i64,
) -> Result<Option<HealthCode>, RegistryError> {
    let (minute_start, minute_messages, minute_signals, hour_start, hour_profiles): (
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = transaction.query_row(
        "SELECT minute_window_start, minute_messages, minute_signals,
                hour_window_start, hour_profiles
         FROM health_peers WHERE node_id = ?1",
        params![state.node_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let minute_expired = now.saturating_sub(minute_start) >= RATE_MINUTE_WINDOW_SECONDS;
    let minute_messages = if minute_expired { 0 } else { minute_messages };
    let minute_signals = if minute_expired { 0 } else { minute_signals };
    let hour_profiles = if now.saturating_sub(hour_start) >= RATE_HOUR_WINDOW_SECONDS {
        0
    } else {
        hour_profiles
    };
    if minute_messages >= MAX_MESSAGES_PER_PEER_PER_MINUTE + RATE_BURST_ALLOWANCE {
        return Ok(Some(HealthCode::RateLimited));
    }
    match kind {
        HealthKind::Profile if hour_profiles >= MAX_PROFILES_PER_PEER_PER_HOUR => {
            return Ok(Some(HealthCode::RateLimited))
        }
        HealthKind::Signal if minute_signals >= MAX_SIGNALS_PER_PEER_PER_MINUTE => {
            return Ok(Some(HealthCode::RateLimited))
        }
        HealthKind::Pulse => {
            if let Some(last_pulse_at) = state.last_pulse_at {
                if now.saturating_sub(last_pulse_at) < MIN_PULSE_INTERVAL_SECONDS {
                    return Ok(Some(HealthCode::RateLimited));
                }
            }
        }
        _ => {}
    }
    Ok(None)
}

fn count_rate(
    transaction: &Transaction<'_>,
    node_id: &str,
    kind: HealthKind,
    now: i64,
) -> Result<(), RegistryError> {
    transaction.execute(
        "UPDATE health_peers
         SET minute_messages = CASE
               WHEN ?2 - minute_window_start >= ?3 THEN 0 ELSE minute_messages END,
             minute_signals = CASE
               WHEN ?2 - minute_window_start >= ?3 THEN 0 ELSE minute_signals END,
             hour_profiles = CASE
               WHEN ?2 - hour_window_start >= ?4 THEN 0 ELSE hour_profiles END,
             minute_window_start = CASE
               WHEN ?2 - minute_window_start >= ?3 THEN ?2 ELSE minute_window_start END,
             hour_window_start = CASE
               WHEN ?2 - hour_window_start >= ?4 THEN ?2 ELSE hour_window_start END
         WHERE node_id = ?1",
        params![
            node_id,
            now,
            RATE_MINUTE_WINDOW_SECONDS,
            RATE_HOUR_WINDOW_SECONDS
        ],
    )?;
    transaction.execute(
        "UPDATE health_peers
         SET minute_messages = minute_messages + 1,
             minute_signals = minute_signals + ?2,
             hour_profiles = hour_profiles + ?3,
             updated_at = ?4
         WHERE node_id = ?1",
        params![
            node_id,
            i64::from(kind == HealthKind::Signal),
            i64::from(kind == HealthKind::Profile),
            now
        ],
    )?;
    Ok(())
}
