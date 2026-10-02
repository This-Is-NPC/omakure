use super::error::RegistryError;
use super::{HEALTH_PLANE_ENABLED, NodeRegistry, SCHEMA_VERSION};
use rusqlite::{Transaction, params};

pub(super) fn create_schema(
    transaction: &Transaction<'_>,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    transaction.execute_batch(SCHEMA)?;
    transaction.execute(
        "INSERT INTO metadata (key, value) VALUES
            ('schema_version', ?1),
            ('node_id', ?2),
            ('public_key_encoding', 'x-only-bip340-hex-lowercase'),
            ('health_plane', ?3)",
        params![
            SCHEMA_VERSION.to_string(),
            registry.local_node_id.as_str(),
            HEALTH_PLANE_ENABLED
        ],
    )?;
    transaction.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    Ok(())
}

/// The complete current schema.
const SCHEMA: &str = "
    CREATE TABLE metadata (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    );
    CREATE TABLE peers (
      node_id TEXT PRIMARY KEY,
      public_key TEXT NOT NULL UNIQUE,
      role TEXT NOT NULL,
      state TEXT NOT NULL,
      capabilities_json TEXT NOT NULL,
      added_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      last_seen TEXT NULL,
      source TEXT NOT NULL
    );
    CREATE TABLE revocations (
      id INTEGER PRIMARY KEY,
      node_id TEXT NOT NULL,
      public_key TEXT NOT NULL,
      revoked_at TEXT NOT NULL,
      reason TEXT NOT NULL,
      replacement_node_id TEXT NULL
    );
    CREATE TABLE audit_events (
      id INTEGER PRIMARY KEY,
      event_type TEXT NOT NULL,
      node_id TEXT NOT NULL,
      from_state TEXT NULL,
      to_state TEXT NULL,
      actor TEXT NOT NULL,
      reason TEXT NOT NULL,
      occurred_at TEXT NOT NULL
    );
    CREATE TABLE replay_keys (
      key TEXT PRIMARY KEY,
      first_seen TEXT NOT NULL,
      expires_at TEXT NOT NULL
    );
    -- Declared, integrity-checked and exported, and deliberately never
    -- written. It was sketched for remote Cues before that plane existed;
    -- when Cues shipped, the design refused it, because it would have been
    -- eight states re-implementing `RunState` and the durable at-most-once
    -- guarantee already falls out of `runs.run_id` being a primary key
    -- derived from the cue id. Left in place rather than dropped: removing
    -- it costs a registry schema bump, and this comment costs nothing.
    -- If you are looking for where a Cue is recorded, it is the runs table.
    CREATE TABLE inbox (
      cue_id TEXT PRIMARY KEY,
      state TEXT NOT NULL,
      received_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      expires_at TEXT NOT NULL,
      outcome_hash TEXT NULL
    );
    CREATE INDEX peers_state_idx ON peers(state);
    CREATE INDEX audit_events_node_idx ON audit_events(node_id, id);
    CREATE TRIGGER revocations_no_update BEFORE UPDATE ON revocations
    BEGIN SELECT RAISE(ABORT, 'revocations are append-only'); END;
    CREATE TRIGGER revocations_no_delete BEFORE DELETE ON revocations
    BEGIN SELECT RAISE(ABORT, 'revocations are append-only'); END;
    CREATE TRIGGER audit_events_no_update BEFORE UPDATE ON audit_events
    BEGIN SELECT RAISE(ABORT, 'audit events are append-only'); END;
    CREATE TRIGGER audit_events_no_delete BEFORE DELETE ON audit_events
    BEGIN SELECT RAISE(ABORT, 'audit events are append-only'); END;

    CREATE TABLE remote_identities (
      node_id TEXT PRIMARY KEY CHECK (length(CAST(node_id AS BLOB)) = 69),
      identity_key BLOB NOT NULL UNIQUE CHECK (length(identity_key) = 32),
      state TEXT NOT NULL CHECK (state IN ('authenticated_untrusted', 'active', 'revoked')),
      first_seen INTEGER NOT NULL CHECK (first_seen > 0),
      revoked_at INTEGER NULL CHECK (revoked_at IS NULL OR revoked_at >= first_seen)
    );
    CREATE TABLE trusted_peers (
      node_id TEXT PRIMARY KEY REFERENCES remote_identities(node_id),
      role INTEGER NOT NULL CHECK (role IN (1, 2)),
      capabilities BLOB NOT NULL CHECK (length(capabilities) <= 4096),
      state TEXT NOT NULL CHECK (state IN ('active', 'revoked')),
      added_at INTEGER NOT NULL CHECK (added_at > 0),
      updated_at INTEGER NOT NULL CHECK (updated_at >= added_at)
    );
    CREATE TABLE transport_key_epochs (
      node_id TEXT NOT NULL REFERENCES remote_identities(node_id),
      key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
      public_key BLOB NOT NULL CHECK (length(public_key) = 32),
      certificate BLOB NOT NULL CHECK (length(certificate) = 245),
      state TEXT NOT NULL CHECK (state IN ('pending', 'active', 'revoked')),
      added_at INTEGER NOT NULL CHECK (added_at > 0),
      retired_at INTEGER NULL CHECK (retired_at IS NULL OR retired_at >= added_at),
      PRIMARY KEY (node_id, key_epoch),
      UNIQUE (node_id, public_key)
    );
    CREATE TABLE channel_sessions (
      session_id BLOB PRIMARY KEY CHECK (length(session_id) = 32),
      node_id TEXT NOT NULL REFERENCES remote_identities(node_id),
      direction INTEGER NOT NULL CHECK (direction IN (0, 1)),
      send_sequence INTEGER NOT NULL CHECK (send_sequence >= 0),
      receive_sequence INTEGER NOT NULL CHECK (receive_sequence >= 0),
      state TEXT NOT NULL CHECK (state IN ('handshaking', 'authenticated_untrusted', 'active', 'closed')),
      started_at INTEGER NOT NULL CHECK (started_at > 0),
      last_seen INTEGER NOT NULL CHECK (last_seen >= started_at),
      expires_at INTEGER NOT NULL CHECK (expires_at >= last_seen)
    );
    CREATE TABLE enrollment_replays (
      replay_kind TEXT NOT NULL CHECK (replay_kind IN ('bundle', 'manual_request')),
      replay_id BLOB NOT NULL CHECK (length(replay_id) = 16),
      expires_at INTEGER NOT NULL CHECK (expires_at > 0),
      first_seen INTEGER NOT NULL CHECK (first_seen > 0),
      PRIMARY KEY (replay_kind, replay_id)
    );
    CREATE TABLE transport_audit (
      id INTEGER PRIMARY KEY,
      event_type TEXT NOT NULL CHECK (length(CAST(event_type AS BLOB)) BETWEEN 1 AND 64),
      node_id TEXT NOT NULL,
      session_id BLOB NULL CHECK (session_id IS NULL OR length(session_id) = 32),
      bundle_id BLOB NULL CHECK (bundle_id IS NULL OR length(bundle_id) = 16),
      direction INTEGER NULL CHECK (direction IS NULL OR direction IN (0, 1)),
      byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
      outcome TEXT NOT NULL CHECK (length(CAST(outcome AS BLOB)) BETWEEN 1 AND 32),
      error_code INTEGER NULL CHECK (error_code IS NULL OR error_code BETWEEN 1000 AND 1999),
      cue_id TEXT NULL CHECK (cue_id IS NULL OR length(cue_id) = 32),
      cue_script TEXT NULL CHECK (cue_script IS NULL OR length(CAST(cue_script AS BLOB)) BETWEEN 1 AND 64),
      cue_reason TEXT NULL CHECK (cue_reason IS NULL OR length(CAST(cue_reason AS BLOB)) BETWEEN 1 AND 128),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0)
    );
    CREATE TABLE cue_rate_limits (
      node_id TEXT PRIMARY KEY,
      window_start INTEGER NOT NULL CHECK (window_start > 0),
      count INTEGER NOT NULL CHECK (count >= 0)
    );
    CREATE INDEX transport_key_epochs_state_idx ON transport_key_epochs(state, node_id);
    CREATE UNIQUE INDEX transport_key_epochs_one_active
      ON transport_key_epochs(node_id) WHERE state = 'active';
    CREATE INDEX channel_sessions_peer_idx ON channel_sessions(node_id, state, last_seen);
    CREATE INDEX enrollment_replays_expiry_idx ON enrollment_replays(expires_at);
    CREATE INDEX transport_audit_node_idx ON transport_audit(node_id, id);
    CREATE INDEX transport_audit_expiry_idx ON transport_audit(occurred_at);
    CREATE TRIGGER trusted_peers_require_known_identity
    BEFORE INSERT ON trusted_peers
    WHEN (SELECT state FROM remote_identities WHERE node_id = NEW.node_id)
      NOT IN ('authenticated_untrusted', 'active')
    BEGIN SELECT RAISE(ABORT, 'trusted peer requires known identity'); END;
    CREATE TRIGGER transport_key_epochs_active_require_trust
    BEFORE INSERT ON transport_key_epochs
    WHEN NEW.state = 'active' AND (
      (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
      OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
    )
    BEGIN SELECT RAISE(ABORT, 'active transport key requires active trusted peer'); END;
    CREATE TRIGGER transport_key_epochs_active_update_require_trust
    BEFORE UPDATE OF state ON transport_key_epochs
    WHEN NEW.state = 'active' AND (
      (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
      OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
    )
    BEGIN SELECT RAISE(ABORT, 'active transport key requires active trusted peer'); END;
    CREATE TRIGGER remote_identities_no_untrusted_trust_update
    BEFORE UPDATE OF state ON remote_identities
    WHEN NEW.state = 'active' AND NOT EXISTS (
      SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id
    )
    BEGIN SELECT RAISE(ABORT, 'active identity requires trusted peer'); END;
    CREATE TRIGGER trusted_peers_no_identity_demotion
    BEFORE UPDATE OF state ON remote_identities
    WHEN NEW.state <> 'active' AND EXISTS (
      SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active'
    )
    BEGIN SELECT RAISE(ABORT, 'trusted peer must be revoked before identity demotion'); END;
    CREATE TRIGGER channel_sessions_active_requires_trust
    BEFORE INSERT ON channel_sessions
    WHEN NEW.state = 'active' AND (
      (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
      OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
    )
    BEGIN SELECT RAISE(ABORT, 'active session requires active trusted peer'); END;
    CREATE TRIGGER channel_sessions_active_update_requires_trust
    BEFORE UPDATE OF state, node_id ON channel_sessions
    WHEN NEW.state = 'active' AND (
      (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
      OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
    )
    BEGIN SELECT RAISE(ABORT, 'active session requires active trusted peer'); END;
    CREATE TRIGGER transport_key_epochs_monotonic_insert
    BEFORE INSERT ON transport_key_epochs
    WHEN NEW.key_epoch <= COALESCE((SELECT MAX(key_epoch) FROM transport_key_epochs WHERE node_id = NEW.node_id), 0)
    BEGIN SELECT RAISE(ABORT, 'transport key epoch must increase'); END;
    CREATE TRIGGER transport_key_epochs_monotonic_update
    BEFORE UPDATE OF key_epoch ON transport_key_epochs
    WHEN NEW.key_epoch <= COALESCE((SELECT MAX(key_epoch) FROM transport_key_epochs WHERE node_id = NEW.node_id AND key_epoch <> OLD.key_epoch), 0)
    BEGIN SELECT RAISE(ABORT, 'transport key epoch must increase'); END;
    CREATE TRIGGER remote_identities_no_delete
    BEFORE DELETE ON remote_identities
    BEGIN SELECT RAISE(ABORT, 'remote identities are retained'); END;
    CREATE TRIGGER trusted_peers_no_delete
    BEFORE DELETE ON trusted_peers
    BEGIN SELECT RAISE(ABORT, 'trusted peer history is retained'); END;
    CREATE TRIGGER transport_key_epochs_no_delete
    BEFORE DELETE ON transport_key_epochs
    BEGIN SELECT RAISE(ABORT, 'transport key epochs are retained'); END;
    CREATE TRIGGER revoked_identity_no_resurrection
    BEFORE UPDATE OF state ON remote_identities
    WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
    BEGIN SELECT RAISE(ABORT, 'revoked identity cannot be resurrected'); END;
    CREATE TRIGGER revoked_trusted_peer_no_resurrection
    BEFORE UPDATE OF state ON trusted_peers
    WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
    BEGIN SELECT RAISE(ABORT, 'revoked trust cannot be resurrected'); END;
    CREATE TRIGGER revoked_transport_epoch_no_resurrection
    BEFORE UPDATE OF state ON transport_key_epochs
    WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
    BEGIN SELECT RAISE(ABORT, 'revoked transport epoch cannot be resurrected'); END;

    CREATE TABLE manual_enrollment_requests (
      request_id BLOB PRIMARY KEY CHECK (length(request_id) = 16),
      request_bytes BLOB NOT NULL CHECK (length(request_bytes) BETWEEN 1 AND 2048),
      request_digest BLOB NOT NULL CHECK (length(request_digest) = 32),
      code_hash BLOB NOT NULL CHECK (length(code_hash) = 32),
      node_id TEXT NOT NULL CHECK (length(CAST(node_id AS BLOB)) = 69),
      identity_key BLOB NOT NULL CHECK (length(identity_key) = 32),
      transport_key BLOB NOT NULL CHECK (length(transport_key) = 32),
      role INTEGER NOT NULL CHECK (role IN (1, 2)),
      capabilities BLOB NOT NULL CHECK (length(capabilities) <= 4096),
      request_created_at INTEGER NOT NULL CHECK (request_created_at > 0),
      request_expires_at INTEGER NOT NULL CHECK (request_expires_at > request_created_at),
      certificate BLOB NOT NULL CHECK (length(certificate) = 245),
      certificate_digest BLOB NOT NULL CHECK (length(certificate_digest) = 32),
      certificate_id BLOB NOT NULL CHECK (length(certificate_id) = 16),
      key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
      not_before INTEGER NOT NULL CHECK (not_before > 0),
      not_after INTEGER NOT NULL CHECK (not_after > not_before),
      state TEXT NOT NULL CHECK (state IN ('pending', 'approved', 'rejected')),
      source TEXT NOT NULL CHECK (source = 'manual'),
      staged_at INTEGER NOT NULL CHECK (staged_at > 0),
      resolved_at INTEGER NULL CHECK (resolved_at IS NULL OR resolved_at >= staged_at),
      pairing_id BLOB NULL CHECK (pairing_id IS NULL OR length(pairing_id) = 16)
    );
    CREATE TABLE enrollment_audits (
      id INTEGER PRIMARY KEY,
      event_code TEXT NOT NULL CHECK (length(CAST(event_code AS BLOB)) BETWEEN 1 AND 64),
      request_id BLOB NULL CHECK (request_id IS NULL OR length(request_id) = 16),
      request_digest BLOB NULL CHECK (request_digest IS NULL OR length(request_digest) = 32),
      node_id TEXT NOT NULL,
      outcome TEXT NOT NULL CHECK (length(CAST(outcome AS BLOB)) BETWEEN 1 AND 32),
      detail TEXT NOT NULL CHECK (length(CAST(detail AS BLOB)) <= 256),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0)
    );
    CREATE INDEX manual_enrollment_requests_state_idx
      ON manual_enrollment_requests(state, node_id);
    CREATE INDEX manual_enrollment_requests_pairing_idx
      ON manual_enrollment_requests(pairing_id);
    CREATE INDEX enrollment_audits_node_idx ON enrollment_audits(node_id, id);
    CREATE INDEX enrollment_audits_request_idx ON enrollment_audits(request_id, id);
    CREATE TRIGGER manual_enrollment_request_immutable
    BEFORE UPDATE ON manual_enrollment_requests
    WHEN NEW.request_id <> OLD.request_id
      OR COALESCE(NEW.pairing_id, X'') <> COALESCE(OLD.pairing_id, X'')
      OR NEW.request_bytes <> OLD.request_bytes
      OR NEW.request_digest <> OLD.request_digest
      OR NEW.code_hash <> OLD.code_hash
      OR NEW.node_id <> OLD.node_id
      OR NEW.identity_key <> OLD.identity_key
      OR NEW.transport_key <> OLD.transport_key
      OR NEW.role <> OLD.role
      OR NEW.capabilities <> OLD.capabilities
      OR NEW.request_created_at <> OLD.request_created_at
      OR NEW.request_expires_at <> OLD.request_expires_at
      OR NEW.certificate <> OLD.certificate
      OR NEW.certificate_digest <> OLD.certificate_digest
      OR NEW.certificate_id <> OLD.certificate_id
      OR NEW.key_epoch <> OLD.key_epoch
      OR NEW.not_before <> OLD.not_before
      OR NEW.not_after <> OLD.not_after
      OR NEW.source <> OLD.source
      OR NEW.staged_at <> OLD.staged_at
    BEGIN SELECT RAISE(ABORT, 'manual enrollment evidence is immutable'); END;
    CREATE TRIGGER enrollment_audits_no_update
    BEFORE UPDATE ON enrollment_audits
    BEGIN SELECT RAISE(ABORT, 'enrollment audits are append-only'); END;
    CREATE TRIGGER enrollment_audits_no_delete
    BEFORE DELETE ON enrollment_audits
    BEGIN SELECT RAISE(ABORT, 'enrollment audits are append-only'); END;

    CREATE TABLE bootstrap_proofs (
      target_node_id TEXT NOT NULL CHECK (length(CAST(target_node_id AS BLOB)) = 69),
      organization TEXT NOT NULL CHECK (length(CAST(organization AS BLOB)) BETWEEN 1 AND 128),
      token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
      nonce_hash BLOB NOT NULL CHECK (length(nonce_hash) = 32),
      expires_at INTEGER NOT NULL CHECK (expires_at > 0),
      consumed_at INTEGER NULL CHECK (consumed_at IS NULL OR consumed_at > 0),
      bundle_id BLOB NULL CHECK (bundle_id IS NULL OR length(bundle_id) = 16),
      cleanup_state TEXT NULL
        CHECK (cleanup_state IS NULL OR cleanup_state IN ('pending', 'complete')),
      PRIMARY KEY (target_node_id, organization, token_hash, nonce_hash)
    );
    CREATE INDEX bootstrap_proofs_expiry_idx ON bootstrap_proofs(expires_at);
    CREATE TRIGGER bootstrap_proofs_no_update
    BEFORE UPDATE ON bootstrap_proofs
    WHEN NEW.target_node_id <> OLD.target_node_id
      OR NEW.organization <> OLD.organization
      OR NEW.token_hash <> OLD.token_hash
      OR NEW.nonce_hash <> OLD.nonce_hash
      OR NEW.expires_at <> OLD.expires_at
      OR (OLD.consumed_at IS NOT NULL AND (
           NEW.consumed_at IS NOT OLD.consumed_at
           OR NEW.bundle_id IS NOT OLD.bundle_id
           OR NEW.cleanup_state IS NULL
           OR NEW.cleanup_state NOT IN ('pending', 'complete')
           OR (OLD.cleanup_state = 'complete' AND NEW.cleanup_state <> OLD.cleanup_state)
         ))
      OR (OLD.consumed_at IS NULL AND (
           NEW.consumed_at IS NULL OR NEW.bundle_id IS NULL
           OR (NEW.cleanup_state IS NOT NULL AND NEW.cleanup_state <> 'pending')
         ))
    BEGIN SELECT RAISE(ABORT, 'bootstrap proof identity is immutable'); END;

    CREATE TABLE health_peers (
      node_id TEXT PRIMARY KEY REFERENCES remote_identities(node_id),
      role INTEGER NOT NULL CHECK (role IN (1, 2)),
      cursor INTEGER NOT NULL DEFAULT 0 CHECK (cursor >= 0),
      last_profile_revision INTEGER NOT NULL DEFAULT 0 CHECK (last_profile_revision >= 0),
      last_pulse_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_pulse_sequence >= 0),
      last_pulse_at INTEGER NULL,
      version_incompatible_at INTEGER NULL,
      minute_window_start INTEGER NOT NULL DEFAULT 0,
      minute_messages INTEGER NOT NULL DEFAULT 0 CHECK (minute_messages >= 0),
      minute_signals INTEGER NOT NULL DEFAULT 0 CHECK (minute_signals >= 0),
      hour_window_start INTEGER NOT NULL DEFAULT 0,
      hour_profiles INTEGER NOT NULL DEFAULT 0 CHECK (hour_profiles >= 0),
      first_seen INTEGER NOT NULL CHECK (first_seen > 0),
      updated_at INTEGER NOT NULL CHECK (updated_at >= first_seen)
    );
    CREATE TABLE health_profiles (
      node_id TEXT PRIMARY KEY REFERENCES health_peers(node_id),
      profile_revision INTEGER NOT NULL CHECK (profile_revision >= 1),
      agent_version TEXT NOT NULL CHECK (length(agent_version) BETWEEN 1 AND 32),
      arch TEXT NOT NULL CHECK (arch IN ('x86_64', 'aarch64', 'unknown')),
      capabilities TEXT NOT NULL CHECK (length(capabilities) <= 2048),
      display_name TEXT NOT NULL CHECK (length(display_name) <= 64),
      distro_id TEXT NOT NULL CHECK (length(distro_id) <= 32),
      distro_version TEXT NOT NULL CHECK (length(distro_version) <= 32),
      omarchy_channel TEXT NOT NULL CHECK (omarchy_channel IN ('', 'stable', 'dev')),
      omarchy_version TEXT NOT NULL CHECK (length(omarchy_version) <= 32),
      platform TEXT NOT NULL CHECK (platform IN ('linux', 'macos', 'windows')),
      role TEXT NOT NULL CHECK (role = 'performer'),
      runtimes TEXT NOT NULL CHECK (length(runtimes) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 2112),
      received_at INTEGER NOT NULL CHECK (received_at > 0),
      baseline_id TEXT NOT NULL DEFAULT ''
        CHECK (baseline_id = '' OR length(baseline_id) = 64),
      baseline_observed_id TEXT NOT NULL DEFAULT ''
        CHECK (baseline_observed_id = '' OR length(baseline_observed_id) = 64)
    );
    CREATE TABLE health_pulses (
      node_id TEXT PRIMARY KEY REFERENCES health_peers(node_id),
      sequence INTEGER NOT NULL CHECK (sequence >= 1),
      emitted_at INTEGER NOT NULL CHECK (emitted_at > 0),
      profile_revision INTEGER NOT NULL CHECK (profile_revision >= 0),
      runner_state TEXT NOT NULL
        CHECK (runner_state IN ('idle', 'busy', 'paused', 'degraded', 'stopped')),
      scheduler_state TEXT NOT NULL CHECK (scheduler_state IN ('running', 'disabled')),
      queue_depth INTEGER NOT NULL CHECK (queue_depth BETWEEN 0 AND 65535),
      workers_busy INTEGER NOT NULL CHECK (workers_busy BETWEEN 0 AND 255),
      workers_configured INTEGER NOT NULL CHECK (workers_configured BETWEEN 0 AND 255),
      uptime_seconds INTEGER NOT NULL CHECK (uptime_seconds BETWEEN 0 AND 4294967295),
      last_run TEXT NULL CHECK (last_run IS NULL OR length(last_run) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 1344),
      received_at INTEGER NOT NULL CHECK (received_at > 0)
    );
    CREATE TABLE health_signals (
      node_id TEXT NOT NULL REFERENCES health_peers(node_id),
      signal_id BLOB NOT NULL CHECK (length(signal_id) = 16),
      sequence INTEGER NOT NULL CHECK (sequence >= 1),
      state TEXT NOT NULL CHECK (state IN ('applied', 'held')),
      kind TEXT NOT NULL CHECK (kind IN ('enrolled', 'revoked', 'run-completed')),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0),
      subject TEXT NULL CHECK (subject IS NULL OR length(subject) = 69),
      run TEXT NULL CHECK (run IS NULL OR length(run) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 1088),
      received_at INTEGER NOT NULL CHECK (received_at > 0),
      expires_at INTEGER NOT NULL CHECK (expires_at > received_at),
      PRIMARY KEY (node_id, signal_id),
      UNIQUE (node_id, sequence)
    );
    CREATE TABLE health_outbox (
      signal_id BLOB PRIMARY KEY CHECK (length(signal_id) = 16),
      target_node_id TEXT NOT NULL CHECK (length(target_node_id) = 69),
      sequence INTEGER NOT NULL UNIQUE CHECK (sequence >= 1),
      kind TEXT NOT NULL CHECK (kind IN ('enrolled', 'revoked', 'run-completed')),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0),
      subject TEXT NULL CHECK (subject IS NULL OR length(subject) = 69),
      run TEXT NULL CHECK (run IS NULL OR length(run) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 1088),
      attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 3),
      last_message_id BLOB NULL CHECK (last_message_id IS NULL OR length(last_message_id) = 16),
      enqueued_at INTEGER NOT NULL CHECK (enqueued_at > 0),
      updated_at INTEGER NOT NULL CHECK (updated_at >= enqueued_at),
      expires_at INTEGER NOT NULL CHECK (expires_at > enqueued_at)
    );
    CREATE TABLE health_replay_keys (
      message_id BLOB PRIMARY KEY CHECK (length(message_id) = 16),
      node_id TEXT NOT NULL CHECK (length(node_id) = 69),
      first_seen INTEGER NOT NULL CHECK (first_seen > 0),
      expires_at INTEGER NOT NULL CHECK (expires_at > first_seen)
    );
    CREATE TABLE health_audit (
      id INTEGER PRIMARY KEY,
      event_code TEXT NOT NULL CHECK (length(event_code) BETWEEN 1 AND 64),
      node_id TEXT NOT NULL CHECK (length(node_id) = 69),
      message_kind TEXT NOT NULL CHECK (length(message_kind) BETWEEN 1 AND 64),
      byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
      outcome TEXT NOT NULL CHECK (outcome IN ('accepted', 'held', 'rejected', 'dropped', 'purged')),
      error_code INTEGER NULL CHECK (error_code IS NULL OR error_code BETWEEN 1000 AND 1999),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0)
    );
    CREATE TABLE health_local (
      key TEXT PRIMARY KEY CHECK (key IN ('signal_sequence', 'signals_dropped')),
      value INTEGER NOT NULL CHECK (value >= 0)
    );
    CREATE INDEX health_audit_expiry_idx ON health_audit(occurred_at);
    CREATE INDEX health_outbox_order_idx ON health_outbox(sequence);
    CREATE INDEX health_replay_keys_expiry_idx ON health_replay_keys(expires_at);
    CREATE INDEX health_signals_expiry_idx ON health_signals(expires_at);
    CREATE INDEX health_signals_order_idx ON health_signals(node_id, state, sequence);
    CREATE TRIGGER health_audit_no_update
    BEFORE UPDATE ON health_audit
    BEGIN SELECT RAISE(ABORT, 'health audit rows are append-only'); END;
    CREATE TRIGGER health_peers_require_active_trust
    BEFORE INSERT ON health_peers
    WHEN NOT EXISTS (
      SELECT 1 FROM trusted_peers t JOIN remote_identities r ON r.node_id = t.node_id
      WHERE t.node_id = NEW.node_id AND t.state = 'active' AND r.state = 'active'
    )
    BEGIN SELECT RAISE(ABORT, 'health state requires an active trusted peer'); END;
    INSERT INTO health_local (key, value) VALUES ('signal_sequence', 0), ('signals_dropped', 0);
";
