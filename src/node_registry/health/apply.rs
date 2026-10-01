use super::super::fields::validate_node_id;
use super::super::{NodeRegistry, RegistryError};
use super::audit::record_health_audit_tx;
use super::evaluate::evaluate;
use super::rows::{authorization_in, decode_opaque_id};
use super::types::{HealthApplyRequest, HealthAuthorization};
use crate::domain::health_plane::bounds::{
    MAX_REPLAY_ROWS, REORDER_BUFFER_SECONDS, REPLAY_RETENTION_SECONDS,
    REPLAY_SECURITY_FLOOR_SECONDS, SIGNAL_RETENTION_SECONDS,
};
use crate::domain::health_plane::model::{
    HealthCode, HealthDecision, ProfileSnapshot, PulseSnapshot, SignalRecord,
};
use rusqlite::{params, Transaction, TransactionBehavior};

impl NodeRegistry {
    /// The one new read-only projection required by Health Plane
    /// authorization: `role` and `capabilities` from `trusted_peers` alongside
    /// the identity and trust state. It creates nothing and mutates nothing.
    pub fn health_authorization(
        &self,
        node_id: &str,
    ) -> Result<Option<HealthAuthorization>, RegistryError> {
        validate_node_id(node_id)?;
        self.with_connection(|connection| authorization_in(connection, node_id))
    }

    /// Apply receive-order steps 7 through 15 in exactly one transaction.
    ///
    /// Every rejection leaves identity, trust, revocation, transport session,
    /// and run state untouched, and writes one redacted audit row.
    pub(crate) fn apply_health_message(
        &self,
        request: HealthApplyRequest<'_>,
    ) -> Result<HealthDecision, RegistryError> {
        validate_node_id(request.sender)?;
        let kind = request.payload.body.kind();
        if kind
            .max_stored_bytes()
            .is_some_and(|cap| request.message_bytes > cap)
        {
            return Ok(HealthDecision::Rejected(HealthCode::MessageTooLarge));
        }
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let decision = evaluate(&transaction, &request)?;
            record_health_audit_tx(
                &transaction,
                kind.wire(),
                request.sender,
                kind.wire(),
                request.message_bytes,
                decision.outcome(),
                decision.code().map(HealthCode::code),
                request.now,
            )?;
            transaction.commit()?;
            Ok(decision)
        })
    }

    /// Mark a peer as speaking an unsupported Health Plane version.
    pub(crate) fn mark_health_version_incompatible(
        &self,
        node_id: &str,
        now: i64,
    ) -> Result<(), RegistryError> {
        validate_node_id(node_id)?;
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute(
                "UPDATE health_peers SET version_incompatible_at = ?2, updated_at = ?2
                 WHERE node_id = ?1",
                params![node_id, now],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }
}

pub(super) fn record_replay_key(
    transaction: &Transaction<'_>,
    message_id: &[u8],
    node_id: &str,
    now: i64,
) -> Result<bool, RegistryError> {
    let mut rows: i64 =
        transaction.query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
            row.get(0)
        })?;
    while rows >= MAX_REPLAY_ROWS {
        let floor = now.saturating_sub(REPLAY_SECURITY_FLOOR_SECONDS);
        let removed = transaction.execute(
            "DELETE FROM health_replay_keys WHERE message_id = (
               SELECT message_id FROM health_replay_keys
               WHERE first_seen <= ?1 ORDER BY expires_at, message_id LIMIT 1
             )",
            params![floor],
        )?;
        if removed == 0 {
            return Ok(false);
        }
        rows -= removed as i64;
    }
    transaction.execute(
        "INSERT INTO health_replay_keys (message_id, node_id, first_seen, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            message_id,
            node_id,
            now,
            now.saturating_add(REPLAY_RETENTION_SECONDS)
        ],
    )?;
    Ok(true)
}

pub(super) fn store_profile(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
    profile: &ProfileSnapshot,
) -> Result<(), RegistryError> {
    let capabilities = serde_json::to_string(&profile.capabilities)
        .map_err(|_| RegistryError::InvalidInput("profile capabilities".to_string()))?;
    let runtimes = serde_json::to_string(&profile.runtimes)
        .map_err(|_| RegistryError::InvalidInput("profile runtimes".to_string()))?;
    transaction.execute(
        "INSERT INTO health_profiles
         (node_id, profile_revision, agent_version, arch, capabilities, display_name,
          distro_id, distro_version, omarchy_channel, omarchy_version, platform, role,
          runtimes, message_bytes, received_at, baseline_id, baseline_observed_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT(node_id) DO UPDATE SET
           profile_revision = excluded.profile_revision,
           agent_version = excluded.agent_version,
           arch = excluded.arch,
           capabilities = excluded.capabilities,
           display_name = excluded.display_name,
           distro_id = excluded.distro_id,
           distro_version = excluded.distro_version,
           omarchy_channel = excluded.omarchy_channel,
           omarchy_version = excluded.omarchy_version,
           platform = excluded.platform,
           role = excluded.role,
           runtimes = excluded.runtimes,
           message_bytes = excluded.message_bytes,
           received_at = excluded.received_at,
           baseline_id = excluded.baseline_id,
           baseline_observed_id = excluded.baseline_observed_id",
        params![
            request.sender,
            profile.profile_revision as i64,
            profile.agent_version,
            profile.arch,
            capabilities,
            profile.display_name,
            profile.distro_id,
            profile.distro_version,
            profile.omarchy_channel,
            profile.omarchy_version,
            profile.platform,
            profile.role,
            runtimes,
            request.message_bytes,
            request.now,
            profile.baseline_id,
            profile.baseline_observed_id,
        ],
    )?;
    Ok(())
}

pub(super) fn store_pulse(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
    pulse: &PulseSnapshot,
) -> Result<(), RegistryError> {
    let last_run = pulse
        .last_run
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| RegistryError::InvalidInput("pulse last run".to_string()))?;
    transaction.execute(
        "INSERT INTO health_pulses
         (node_id, sequence, emitted_at, profile_revision, runner_state, scheduler_state,
          queue_depth, workers_busy, workers_configured, uptime_seconds, last_run,
          message_bytes, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(node_id) DO UPDATE SET
           sequence = excluded.sequence,
           emitted_at = excluded.emitted_at,
           profile_revision = excluded.profile_revision,
           runner_state = excluded.runner_state,
           scheduler_state = excluded.scheduler_state,
           queue_depth = excluded.queue_depth,
           workers_busy = excluded.workers_busy,
           workers_configured = excluded.workers_configured,
           uptime_seconds = excluded.uptime_seconds,
           last_run = excluded.last_run,
           message_bytes = excluded.message_bytes,
           received_at = excluded.received_at",
        params![
            request.sender,
            pulse.sequence as i64,
            pulse.emitted_at,
            pulse.profile_revision as i64,
            pulse.runner.state,
            pulse.runner.scheduler,
            pulse.runner.queue_depth as i64,
            pulse.runner.workers_busy as i64,
            pulse.runner.workers_configured as i64,
            pulse.uptime_seconds as i64,
            last_run,
            request.message_bytes,
            request.now,
        ],
    )?;
    Ok(())
}

pub(super) fn store_signal(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
    signal: &SignalRecord,
    hold: bool,
) -> Result<(), RegistryError> {
    let signal_id = decode_opaque_id(&signal.signal_id)?;
    let run = signal
        .run
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| RegistryError::InvalidInput("signal run".to_string()))?;
    let lifetime = if hold {
        REORDER_BUFFER_SECONDS
    } else {
        SIGNAL_RETENTION_SECONDS
    };
    transaction.execute(
        "INSERT INTO health_signals
         (node_id, signal_id, sequence, state, kind, occurred_at, subject, run,
          message_bytes, received_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            request.sender,
            signal_id,
            signal.sequence as i64,
            if hold { "held" } else { "applied" },
            signal.kind.wire(),
            signal.occurred_at,
            signal.subject,
            run,
            request.message_bytes,
            request.now,
            request.now.saturating_add(lifetime),
        ],
    )?;
    Ok(())
}

/// Advance the cursor to `sequence` and promote every contiguous held Signal.
pub(super) fn advance_cursor(
    transaction: &Transaction<'_>,
    node_id: &str,
    sequence: u64,
) -> Result<u64, RegistryError> {
    let mut cursor = sequence;
    loop {
        let next = cursor.saturating_add(1) as i64;
        let promoted = transaction.execute(
            "UPDATE health_signals
             SET state = 'applied', expires_at = received_at + ?3
             WHERE node_id = ?1 AND sequence = ?2 AND state = 'held'",
            params![node_id, next, SIGNAL_RETENTION_SECONDS],
        )?;
        if promoted == 0 {
            break;
        }
        cursor = next as u64;
    }
    transaction.execute(
        "UPDATE health_peers SET cursor = ?2 WHERE node_id = ?1",
        params![node_id, cursor as i64],
    )?;
    Ok(cursor)
}
