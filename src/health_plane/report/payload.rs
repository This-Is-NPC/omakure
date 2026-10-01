use super::{ProfileFacts, PulseFacts};
use crate::health_plane::bounds::{
    HEALTH_VERSION, MAX_SAFE_INTEGER, MAX_STORED_SIGNAL_BYTES, OPAQUE_ID_HEX_CHARS, SIGNATURE_BYTES,
};
use crate::health_plane::model::{HealthCode, HealthKind, SignalRecord};
use serde_json::{json, Value};

/// The fixed-width envelope fields used when measuring a Signal's real size.
///
/// Every direct-envelope field except `payload` is constant width: `nonce` is
/// 32 hex characters, `session_id` is 64, `sender` is a 69-byte node ID,
/// `version` is one digit, and `created_at` is a ten-digit Unix second until
/// the year 2286. Measuring the canonical bytes of that shape therefore yields
/// the real encoded size without needing a signing key.
const SIZE_PROBE_CREATED_AT: u64 = 1_700_000_000;
const SESSION_ID_HEX_CHARS: usize = 64;

/// The frozen Signal payload object.
///
/// All six `signal` fields are always present and the one that does not apply
/// to this kind is explicitly `null`, because the frozen closed schema rejects
/// an omitted field with `health_unknown_field` (1114). The `run` object
/// carries exactly the five fields the Signal schema names: it has no
/// `started_at` and no `trigger`, which the Pulse `last_run` object does.
pub fn signal_payload(target: &str, message_id: &str, signal: &SignalRecord) -> Value {
    let run = match &signal.run {
        Some(run) => json!({
            "exit_code": run.exit_code,
            "finished_at": run.finished_at,
            "run_id": run.run_id,
            "script": run.script,
            "state": run.state,
        }),
        None => Value::Null,
    };
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "signal": {
            "kind": signal.kind.wire(),
            "occurred_at": signal.occurred_at,
            "run": run,
            "sequence": signal.sequence,
            "signal_id": signal.signal_id,
            "subject": signal.subject,
        }
    })
}

/// The encoded byte count one Signal occupies on the wire and in storage.
///
/// The result is the canonical envelope length plus the 64-byte signature,
/// measured from the real payload, and is clamped into the frozen stored-row
/// range so a hostile fact can never widen the storage accounting.
///
/// The measurement always uses the widest permitted `sequence`, because the
/// outbox assigns the real sequence after the size is recorded. The stored
/// accounting is therefore never optimistic.
pub fn signal_encoded_bytes(target: &str, signal: &SignalRecord) -> i64 {
    let widest = SignalRecord {
        sequence: MAX_SAFE_INTEGER,
        ..signal.clone()
    };
    let payload = signal_payload(target, &"0".repeat(OPAQUE_ID_HEX_CHARS), &widest);
    let mut object = serde_json::Map::new();
    object.insert("created_at".into(), Value::from(SIZE_PROBE_CREATED_AT));
    object.insert("kind".into(), Value::from(HealthKind::Signal.wire()));
    object.insert("nonce".into(), Value::from("0".repeat(OPAQUE_ID_HEX_CHARS)));
    object.insert("payload".into(), payload);
    object.insert("sender".into(), Value::from(target));
    object.insert(
        "session_id".into(),
        Value::from("0".repeat(SESSION_ID_HEX_CHARS)),
    );
    object.insert("version".into(), Value::from(1_u8));
    let canonical = serde_jcs::to_vec(&Value::Object(object)).unwrap_or_default();
    let bytes = canonical.len().saturating_add(SIGNATURE_BYTES) as i64;
    bytes.clamp(1, MAX_STORED_SIGNAL_BYTES)
}

/// The frozen Profile payload object.
pub(super) fn profile_payload(
    target: &str,
    message_id: &str,
    facts: &ProfileFacts,
    profile_revision: u64,
) -> Value {
    let runtimes: Vec<Value> = facts
        .runtimes
        .iter()
        .map(|runtime| {
            json!({
                "available": runtime.available,
                "name": runtime.name,
                "version": runtime.version,
            })
        })
        .collect();
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "profile": {
            "agent_version": facts.agent_version,
            "arch": facts.arch,
            "baseline_id": facts.baseline_id,
            "baseline_observed_id": facts.baseline_observed_id,
            "capabilities": facts.capabilities,
            "display_name": facts.display_name,
            "distro_id": facts.distro_id,
            "distro_version": facts.distro_version,
            "omarchy_channel": facts.omarchy_channel,
            "omarchy_version": facts.omarchy_version,
            "platform": facts.platform,
            "profile_revision": profile_revision,
            "role": "performer",
            "runtimes": runtimes,
        }
    })
}

/// The frozen Pulse payload object.
pub(super) fn pulse_payload(
    target: &str,
    message_id: &str,
    facts: &PulseFacts,
    profile_revision: u64,
    sequence: u64,
) -> Value {
    let last_run = match &facts.last_run {
        Some(run) => json!({
            "exit_code": run.exit_code,
            "finished_at": run.finished_at,
            "run_id": run.run_id,
            "script": run.script,
            "started_at": run.started_at.unwrap_or(run.finished_at),
            "state": run.state,
            "trigger": run.trigger.clone().unwrap_or_else(|| "manual".to_string()),
        }),
        None => Value::Null,
    };
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "pulse": {
            "emitted_at": sequence,
            "last_run": last_run,
            "profile_revision": profile_revision,
            "runner": {
                "queue_depth": facts.runner.queue_depth,
                "scheduler": facts.runner.scheduler,
                "state": facts.runner.state,
                "workers_busy": facts.runner.workers_busy,
                "workers_configured": facts.runner.workers_configured,
            },
            "sequence": sequence,
            "uptime_seconds": facts.uptime_seconds,
        }
    })
}

/// The frozen acknowledgement payload object.
pub fn ack_payload(target: &str, message_id: &str, acked_message_id: &str, cursor: u64) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "ack": {
            "accepted": true,
            "acked_message_id": acked_message_id,
            "cursor": cursor,
        }
    })
}

/// The frozen rejection payload object. It carries only the stable code and
/// its name; never the offending bytes, a field value, or a diagnostic string.
pub fn error_payload(
    target: &str,
    message_id: &str,
    acked_message_id: &str,
    code: HealthCode,
) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "error": {
            "accepted": false,
            "acked_message_id": acked_message_id,
            "code": code.code(),
            "reason": code.name(),
        }
    })
}
