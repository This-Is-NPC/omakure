//! Strict closed-schema validation for Health Plane payloads.
//!
//! Version 1 has no forward-compatible "ignore unknown fields" behaviour.
//! Every field name, type, range, grammar, ordering, and combination is
//! checked, and the receiver rejects rather than redacts so a redaction bug
//! can never silently persist sensitive data.

use super::bounds::{
    BASELINE_ID_HEX_CHARS, CAPABILITY_ALLOWLIST, MAX_AGENT_VERSION_BYTES, MAX_ARRAY_LENGTH,
    MAX_CAPABILITY_BYTES, MAX_CAPABILITY_COUNT, MAX_DISPLAY_NAME_BYTES, MAX_DISTRO_ID_BYTES,
    MAX_DISTRO_VERSION_BYTES, MAX_EXIT_CODE, MAX_FIELD_NAME_BYTES, MAX_JSON_DEPTH,
    MAX_PAYLOAD_FIELDS, MAX_QUEUE_DEPTH, MAX_RUNTIME_COUNT, MAX_SAFE_INTEGER, MAX_SCRIPT_BYTES,
    MAX_STRING_BYTES, MAX_UPTIME_SECONDS, MAX_WORKERS, MIN_EXIT_CODE, NODE_ID_BYTES,
    OPAQUE_ID_HEX_CHARS, RUNTIME_NAMES,
};
use super::model::{
    AckBody, ErrorBody, HealthBody, HealthCode, HealthKind, HealthPayload, ProfileSnapshot,
    PulseSnapshot, RunFact, RunnerFact, RuntimeFact, SignalKind, SignalRecord,
};
use serde_json::{Map, Value};

const RUN_STATES: [&str; 5] = [
    "completed",
    "failed",
    "cancelled",
    "timed_out",
    "dead_letter",
];
const RUN_TRIGGERS: [&str; 4] = ["manual", "scheduled", "queue", "cue"];
const RUNNER_STATES: [&str; 5] = ["idle", "busy", "paused", "degraded", "stopped"];
const SCHEDULER_STATES: [&str; 2] = ["running", "disabled"];
const ARCHITECTURES: [&str; 3] = ["x86_64", "aarch64", "unknown"];
const PLATFORMS: [&str; 3] = ["linux", "macos", "windows"];
const OMARCHY_CHANNELS: [&str; 3] = ["", "stable", "dev"];

/// Validate the `health_version` field alone, which is receive-order step 4
/// and precedes closed-schema validation.
pub fn validate_version(payload: &Value) -> Result<(), HealthCode> {
    let object = payload.as_object().ok_or(HealthCode::InvalidMessage)?;
    match object.get("health_version").and_then(Value::as_u64) {
        None => Err(HealthCode::UnknownField),
        Some(version) if version != super::bounds::HEALTH_VERSION => {
            Err(HealthCode::UnsupportedVersion)
        }
        Some(_) => Ok(()),
    }
}

/// Read `payload.target` without running full schema validation. Used by the
/// mixed-version path, which must decide whether a `health_error` reply is
/// permitted before it can trust any other field.
pub fn peek_target(payload: &Value) -> Option<String> {
    let object = payload.as_object()?;
    node_id_field(object.get("target")).ok()
}

/// Validate one complete Health Plane payload against the frozen closed schema.
pub fn validate_payload(
    kind: HealthKind,
    payload: &Value,
    created_at: i64,
) -> Result<HealthPayload, HealthCode> {
    let object = payload.as_object().ok_or(HealthCode::InvalidMessage)?;
    structural_bounds(payload)?;
    exact_fields(
        object,
        &["health_version", "message_id", "target", kind.body_field()],
    )?;
    let message_id = hex16(object.get("message_id"))?;
    let target = node_id_field(object.get("target"))?;
    let body = validate_body(kind, &object[kind.body_field()], created_at)?;
    Ok(HealthPayload {
        message_id,
        target,
        body,
    })
}

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> Result<(), HealthCode> {
    if object.len() != expected.len() {
        return Err(HealthCode::UnknownField);
    }
    for name in expected {
        if !object.contains_key(*name) {
            return Err(HealthCode::UnknownField);
        }
    }
    Ok(())
}

fn structural_bounds(payload: &Value) -> Result<(), HealthCode> {
    fn walk(value: &Value, depth: usize, fields: &mut usize) -> Result<(), HealthCode> {
        if depth > MAX_JSON_DEPTH {
            return Err(HealthCode::InvalidMessage);
        }
        match value {
            Value::Object(map) => {
                for (name, child) in map {
                    if name.len() > MAX_FIELD_NAME_BYTES {
                        return Err(HealthCode::UnknownField);
                    }
                    *fields += 1;
                    if *fields > MAX_PAYLOAD_FIELDS {
                        return Err(HealthCode::InvalidMessage);
                    }
                    walk(child, depth + 1, fields)?;
                }
                Ok(())
            }
            Value::Array(items) => {
                if items.len() > MAX_ARRAY_LENGTH {
                    return Err(HealthCode::InvalidMessage);
                }
                for item in items {
                    walk(item, depth + 1, fields)?;
                }
                Ok(())
            }
            Value::String(text) => {
                if text.len() > MAX_STRING_BYTES
                    || text.starts_with("secret://")
                    || text.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
                {
                    return Err(HealthCode::InvalidMessage);
                }
                Ok(())
            }
            Value::Number(number) => {
                if number.is_f64() {
                    return Err(HealthCode::InvalidMessage);
                }
                Ok(())
            }
            Value::Bool(_) | Value::Null => Ok(()),
        }
    }
    // The payload sits one level inside the envelope object, so it starts at
    // depth 1 and the frozen depth limit of 5 still applies unchanged.
    let mut fields = 0;
    walk(payload, 1, &mut fields)
}

fn grammar(text: &str, max: usize, allow_empty: bool, extra: &str) -> bool {
    if text.is_empty() {
        return allow_empty;
    }
    if text.len() > max {
        return false;
    }
    let first = text.as_bytes()[0];
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    text.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || extra.as_bytes().contains(&byte))
}

fn hex16(value: Option<&Value>) -> Result<String, HealthCode> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if text.len() != OPAQUE_ID_HEX_CHARS || !text.bytes().all(is_lower_hex) {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(text.to_string())
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

/// A baseline identity as the Profile carries it: empty, or the exact width
/// `crate::baseline` derives.
///
/// Empty is the only way to say "this node holds no baseline", and it is a
/// separate answer from any identity — an empty entry list is not signable, so
/// no baseline that was ever pushed can name itself with the empty string.
fn baseline_id_field(value: Option<&Value>) -> Result<String, HealthCode> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !text.is_empty() && (text.len() != BASELINE_ID_HEX_CHARS || !text.bytes().all(is_lower_hex))
    {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(text.to_string())
}

fn node_id_field(value: Option<&Value>) -> Result<String, HealthCode> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if text.len() != NODE_ID_BYTES
        || !text.starts_with("omk1_")
        || !text[5..].bytes().all(is_lower_hex)
    {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(text.to_string())
}

fn bounded_u64(value: Option<&Value>, min: u64, max: u64) -> Result<u64, HealthCode> {
    let number = value
        .and_then(Value::as_u64)
        .ok_or(HealthCode::InvalidMessage)?;
    if number < min || number > max {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(number)
}

fn one_of(value: Option<&Value>, allowed: &[&str]) -> Result<String, HealthCode> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !allowed.contains(&text) {
        return Err(HealthCode::InvalidMessage);
    }
    Ok(text.to_string())
}

fn exit_code(value: Option<&Value>) -> Result<Option<i64>, HealthCode> {
    match value {
        Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let code = number.as_i64().ok_or(HealthCode::InvalidMessage)?;
            if !(MIN_EXIT_CODE..=MAX_EXIT_CODE).contains(&code) {
                return Err(HealthCode::InvalidMessage);
            }
            Ok(Some(code))
        }
        _ => Err(HealthCode::InvalidMessage),
    }
}

fn run_object(value: &Value, with_trigger: bool) -> Result<RunFact, HealthCode> {
    let object = value.as_object().ok_or(HealthCode::InvalidMessage)?;
    let fields: &[&str] = if with_trigger {
        &[
            "exit_code",
            "finished_at",
            "run_id",
            "script",
            "started_at",
            "state",
            "trigger",
        ]
    } else {
        &["exit_code", "finished_at", "run_id", "script", "state"]
    };
    exact_fields(object, fields)?;
    let exit = exit_code(object.get("exit_code"))?;
    let finished_at = bounded_u64(object.get("finished_at"), 1, MAX_SAFE_INTEGER)?;
    let run_id = hex16(object.get("run_id"))?;
    let script = object
        .get("script")
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !grammar(script, MAX_SCRIPT_BYTES, false, "._-") {
        return Err(HealthCode::InvalidMessage);
    }
    let state = one_of(object.get("state"), &RUN_STATES)?;
    let mut started_at = None;
    let mut trigger = None;
    if with_trigger {
        let started = bounded_u64(object.get("started_at"), 1, MAX_SAFE_INTEGER)?;
        if finished_at < started {
            return Err(HealthCode::InvalidMessage);
        }
        started_at = Some(started as i64);
        trigger = Some(one_of(object.get("trigger"), &RUN_TRIGGERS)?);
    }
    Ok(RunFact {
        exit_code: exit,
        finished_at: finished_at as i64,
        run_id,
        script: script.to_string(),
        started_at,
        state,
        trigger,
    })
}

fn validate_body(
    kind: HealthKind,
    body: &Value,
    created_at: i64,
) -> Result<HealthBody, HealthCode> {
    let object = body.as_object().ok_or(HealthCode::InvalidMessage)?;
    match kind {
        HealthKind::Profile => Ok(HealthBody::Profile(validate_profile(object)?)),
        HealthKind::Pulse => Ok(HealthBody::Pulse(validate_pulse(object, created_at)?)),
        HealthKind::Signal => Ok(HealthBody::Signal(validate_signal(object, created_at)?)),
        HealthKind::Ack => {
            exact_fields(object, &["accepted", "acked_message_id", "cursor"])?;
            if object.get("accepted").and_then(Value::as_bool) != Some(true) {
                return Err(HealthCode::InvalidMessage);
            }
            let acked_message_id = hex16(object.get("acked_message_id"))?;
            let cursor = bounded_u64(object.get("cursor"), 0, MAX_SAFE_INTEGER)?;
            Ok(HealthBody::Ack(AckBody {
                accepted: true,
                acked_message_id,
                cursor,
            }))
        }
        HealthKind::Error => {
            exact_fields(object, &["accepted", "acked_message_id", "code", "reason"])?;
            if object.get("accepted").and_then(Value::as_bool) != Some(false) {
                return Err(HealthCode::InvalidMessage);
            }
            let acked_message_id = hex16(object.get("acked_message_id"))?;
            let code = bounded_u64(object.get("code"), 1101, 1115)? as u16;
            let expected = HealthCode::from_code(code).ok_or(HealthCode::InvalidMessage)?;
            let reason = one_of(object.get("reason"), &[expected.name()])?;
            Ok(HealthBody::Error(ErrorBody {
                accepted: false,
                acked_message_id,
                code,
                reason,
            }))
        }
    }
}

fn validate_profile(object: &Map<String, Value>) -> Result<ProfileSnapshot, HealthCode> {
    exact_fields(
        object,
        &[
            "agent_version",
            "arch",
            "baseline_id",
            "baseline_observed_id",
            "capabilities",
            "display_name",
            "distro_id",
            "distro_version",
            "omarchy_channel",
            "omarchy_version",
            "platform",
            "profile_revision",
            "role",
            "runtimes",
        ],
    )?;
    let agent_version = object
        .get("agent_version")
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !grammar(agent_version, MAX_AGENT_VERSION_BYTES, false, ".+-") {
        return Err(HealthCode::InvalidMessage);
    }
    let arch = one_of(object.get("arch"), &ARCHITECTURES)?;
    let baseline_id = baseline_id_field(object.get("baseline_id"))?;
    let baseline_observed_id = baseline_id_field(object.get("baseline_observed_id"))?;
    // A node that records no baseline cannot have observed one, and the pair
    // is what the drift comparison reads. Allowing "nothing installed, this
    // observed" would put a verdict on the wire that no set on disk could
    // justify.
    if baseline_id.is_empty() && !baseline_observed_id.is_empty() {
        return Err(HealthCode::InvalidMessage);
    }
    let entries = object
        .get("capabilities")
        .and_then(Value::as_array)
        .ok_or(HealthCode::InvalidMessage)?;
    if entries.len() > MAX_CAPABILITY_COUNT {
        return Err(HealthCode::InvalidMessage);
    }
    let mut capabilities = Vec::with_capacity(entries.len());
    let mut previous = "";
    for entry in entries {
        let text = entry.as_str().ok_or(HealthCode::InvalidMessage)?;
        if text.len() > MAX_CAPABILITY_BYTES
            || !CAPABILITY_ALLOWLIST.contains(&text)
            || text <= previous
        {
            return Err(HealthCode::InvalidMessage);
        }
        previous = text;
        capabilities.push(text.to_string());
    }
    let display_name = object
        .get("display_name")
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !grammar(display_name, MAX_DISPLAY_NAME_BYTES, true, " ._-") || display_name.ends_with(' ') {
        return Err(HealthCode::InvalidMessage);
    }
    let distro_id = object
        .get("distro_id")
        .and_then(Value::as_str)
        .ok_or(HealthCode::InvalidMessage)?;
    if !grammar(distro_id, MAX_DISTRO_ID_BYTES, true, "._-")
        || distro_id.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(HealthCode::InvalidMessage);
    }
    let mut versions = Vec::with_capacity(2);
    for name in ["distro_version", "omarchy_version"] {
        let text = object
            .get(name)
            .and_then(Value::as_str)
            .ok_or(HealthCode::InvalidMessage)?;
        if !grammar(text, MAX_DISTRO_VERSION_BYTES, true, "._+-") {
            return Err(HealthCode::InvalidMessage);
        }
        versions.push(text.to_string());
    }
    let omarchy_channel = one_of(object.get("omarchy_channel"), &OMARCHY_CHANNELS)?;
    let platform = one_of(object.get("platform"), &PLATFORMS)?;
    let profile_revision = bounded_u64(object.get("profile_revision"), 1, MAX_SAFE_INTEGER)?;
    let role = one_of(object.get("role"), &["performer"])?;
    let entries = object
        .get("runtimes")
        .and_then(Value::as_array)
        .ok_or(HealthCode::InvalidMessage)?;
    if entries.len() > MAX_RUNTIME_COUNT {
        return Err(HealthCode::InvalidMessage);
    }
    let mut runtimes = Vec::with_capacity(entries.len());
    let mut previous_runtime = "";
    for entry in entries {
        let runtime = entry.as_object().ok_or(HealthCode::InvalidMessage)?;
        exact_fields(runtime, &["available", "name", "version"])?;
        let available = runtime
            .get("available")
            .and_then(Value::as_bool)
            .ok_or(HealthCode::InvalidMessage)?;
        let name = one_of(runtime.get("name"), &RUNTIME_NAMES)?;
        if name.as_str() <= previous_runtime {
            return Err(HealthCode::InvalidMessage);
        }
        let version = runtime
            .get("version")
            .and_then(Value::as_str)
            .ok_or(HealthCode::InvalidMessage)?;
        if !grammar(version, MAX_DISTRO_VERSION_BYTES, true, "._+-")
            || (!available && !version.is_empty())
        {
            return Err(HealthCode::InvalidMessage);
        }
        previous_runtime = RUNTIME_NAMES
            .into_iter()
            .find(|candidate| *candidate == name.as_str())
            .ok_or(HealthCode::InvalidMessage)?;
        runtimes.push(RuntimeFact {
            available,
            name,
            version: version.to_string(),
        });
    }
    let mut versions = versions.into_iter();
    Ok(ProfileSnapshot {
        agent_version: agent_version.to_string(),
        arch,
        baseline_id,
        baseline_observed_id,
        capabilities,
        display_name: display_name.to_string(),
        distro_id: distro_id.to_string(),
        distro_version: versions.next().unwrap_or_default(),
        omarchy_channel,
        omarchy_version: versions.next().unwrap_or_default(),
        platform,
        profile_revision,
        role,
        runtimes,
    })
}

fn validate_pulse(
    object: &Map<String, Value>,
    created_at: i64,
) -> Result<PulseSnapshot, HealthCode> {
    exact_fields(
        object,
        &[
            "emitted_at",
            "last_run",
            "profile_revision",
            "runner",
            "sequence",
            "uptime_seconds",
        ],
    )?;
    let emitted_at = bounded_u64(object.get("emitted_at"), 1, MAX_SAFE_INTEGER)?;
    if i64::try_from(emitted_at).map_err(|_| HealthCode::InvalidMessage)? != created_at {
        return Err(HealthCode::InvalidMessage);
    }
    let last_run = match object.get("last_run") {
        Some(Value::Null) => None,
        Some(value) => Some(run_object(value, true)?),
        None => return Err(HealthCode::UnknownField),
    };
    let profile_revision = bounded_u64(object.get("profile_revision"), 0, MAX_SAFE_INTEGER)?;
    let runner = object
        .get("runner")
        .and_then(Value::as_object)
        .ok_or(HealthCode::InvalidMessage)?;
    exact_fields(
        runner,
        &[
            "queue_depth",
            "scheduler",
            "state",
            "workers_busy",
            "workers_configured",
        ],
    )?;
    let queue_depth = bounded_u64(runner.get("queue_depth"), 0, MAX_QUEUE_DEPTH)?;
    let scheduler = one_of(runner.get("scheduler"), &SCHEDULER_STATES)?;
    let state = one_of(runner.get("state"), &RUNNER_STATES)?;
    let workers_busy = bounded_u64(runner.get("workers_busy"), 0, MAX_WORKERS)?;
    let workers_configured = bounded_u64(runner.get("workers_configured"), 0, MAX_WORKERS)?;
    if workers_busy > workers_configured {
        return Err(HealthCode::InvalidMessage);
    }
    let sequence = bounded_u64(object.get("sequence"), 1, MAX_SAFE_INTEGER)?;
    let uptime_seconds = bounded_u64(object.get("uptime_seconds"), 0, MAX_UPTIME_SECONDS)?;
    Ok(PulseSnapshot {
        emitted_at: emitted_at as i64,
        last_run,
        profile_revision,
        runner: RunnerFact {
            queue_depth,
            scheduler,
            state,
            workers_busy,
            workers_configured,
        },
        sequence,
        uptime_seconds,
    })
}

fn validate_signal(
    object: &Map<String, Value>,
    created_at: i64,
) -> Result<SignalRecord, HealthCode> {
    exact_fields(
        object,
        &[
            "kind",
            "occurred_at",
            "run",
            "sequence",
            "signal_id",
            "subject",
        ],
    )?;
    let signal_kind = one_of(
        object.get("kind"),
        &[
            SignalKind::Enrolled.wire(),
            SignalKind::Revoked.wire(),
            SignalKind::RunCompleted.wire(),
        ],
    )?;
    let signal_kind = SignalKind::parse(&signal_kind).ok_or(HealthCode::InvalidMessage)?;
    let occurred_at = bounded_u64(object.get("occurred_at"), 1, MAX_SAFE_INTEGER)?;
    let occurred_at = i64::try_from(occurred_at).map_err(|_| HealthCode::InvalidMessage)?;
    if created_at < occurred_at {
        return Err(HealthCode::InvalidMessage);
    }
    let has_run = !matches!(object.get("run"), Some(Value::Null));
    let has_subject = !matches!(object.get("subject"), Some(Value::Null));
    let mut run = None;
    let mut subject = None;
    match signal_kind {
        SignalKind::RunCompleted if has_run && !has_subject => {
            let fact = run_object(&object["run"], false)?;
            if fact.finished_at != occurred_at {
                return Err(HealthCode::InvalidMessage);
            }
            run = Some(fact);
        }
        SignalKind::Enrolled | SignalKind::Revoked if has_subject && !has_run => {
            subject = Some(node_id_field(object.get("subject"))?);
        }
        _ => return Err(HealthCode::InvalidMessage),
    }
    let sequence = bounded_u64(object.get("sequence"), 1, MAX_SAFE_INTEGER)?;
    let signal_id = hex16(object.get("signal_id"))?;
    Ok(SignalRecord {
        kind: signal_kind,
        occurred_at,
        run,
        sequence,
        signal_id,
        subject,
    })
}

/// Read `payload.message_id` without running full schema validation. Used by
/// the mixed-version path, which must be able to name the message it rejects.
pub fn peek_message_id(payload: &Value) -> Option<String> {
    let object = payload.as_object()?;
    hex16(object.get("message_id")).ok()
}
