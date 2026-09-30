use super::gates::{is_well_formed_cue_id, is_well_formed_script_name};
use super::MAX_REASON_BYTES;
use crate::util::hex;

/// The `cue_dispatch` payload, after shape validation.
///
/// Parsing is total and rejects anything outside the frozen grammar, so every
/// field below is already within its bound by the time a gate reads it. None of
/// them is an authorization input: they say *what* was asked for, never whether
/// it is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CueDispatch {
    pub(super) cue_id: String,
    pub(super) script: String,
    pub(super) not_before: i64,
    pub(super) expires_at: i64,
    pub(super) reason: String,
}

impl CueDispatch {
    pub(super) fn parse(payload: &serde_json::Value) -> Option<Self> {
        let object = payload.as_object()?;
        const FIELDS: &[&str] = &[
            "version",
            "cue_id",
            "script",
            "not_before",
            "expires_at",
            "reason",
        ];
        if object.keys().any(|key| !FIELDS.contains(&key.as_str())) {
            return None;
        }
        if object.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
            return None;
        }
        let cue_id = object.get("cue_id")?.as_str()?.to_string();
        if !is_well_formed_cue_id(&cue_id) {
            return None;
        }
        let script = object.get("script")?.as_str()?.to_string();
        if !is_well_formed_script_name(&script) {
            return None;
        }
        let reason = object.get("reason")?.as_str()?.to_string();
        if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
            return None;
        }
        let not_before = object.get("not_before")?.as_i64()?;
        let expires_at = object.get("expires_at")?.as_i64()?;
        if not_before < 1 || expires_at < 1 {
            return None;
        }
        Some(Self {
            cue_id,
            script,
            not_before,
            expires_at,
            reason,
        })
    }
}

/// What gate E authorized: which file, and exactly which bytes.
///
/// Carried from the gate to the accept transition so the two can be compared.
/// A name is not enough -- the whole point is that the *content* authorized is
/// the content enqueued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScriptBinding {
    pub(super) path: std::path::PathBuf,
    pub(super) content_hash: String,
}

#[derive(Debug, Clone)]
pub(super) struct CueDecisionRecord {
    pub(super) cue_id: String,
    pub(super) decided_at: i64,
    pub(super) reply: Option<Vec<u8>>,
}

/// SHA-256 of a script's bytes, or `None` if it cannot be read.
///
/// Unreadable is not "unchanged": a missing or unreadable file must fail the
/// comparison rather than pass it, so the caller treats `None` as a mismatch.
///
/// Public because the executor's third check must compare against the same
/// digest gate E recorded. Two definitions of "the authorized bytes" would
/// eventually disagree, and the check that disagreed would be the one that
/// silently stopped defending anything.
pub fn content_hash(path: &std::path::Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(hex::encode(&hasher.finalize()))
}
