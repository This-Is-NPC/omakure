use serde::Serialize;
use std::sync::{Arc, OnceLock, RwLock};

/// Structured HTTP audit event. Never includes Authorization or raw tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HttpAuditEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub method: String,
    pub path: String,
    pub outcome: String,
    pub status: u16,
}

#[derive(Debug, Clone)]
pub(super) struct AuditRunId(pub(super) String);

type AuditHook = Arc<dyn Fn(&HttpAuditEvent) + Send + Sync>;

fn audit_hook_slot() -> &'static RwLock<Option<AuditHook>> {
    static SLOT: OnceLock<RwLock<Option<AuditHook>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

pub(super) fn emit_http_audit(event: HttpAuditEvent) {
    if let Ok(guard) = audit_hook_slot().read() {
        guard.iter().for_each(|hook| hook(&event));
    }
    if let Ok(line) = serde_json::to_string(&event) {
        // Operators correlate enqueue/cancel/dead-letter via token_id in this line.
        eprintln!("omakure.http_audit {line}");
    }
}

pub(super) async fn emit_http_audit_async(
    event: HttpAuditEvent,
) -> Result<(), crate::operations::OperationError> {
    static GATE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let gate = Arc::clone(GATE.get_or_init(|| {
        Arc::new(tokio::sync::Semaphore::new(
            super::state::MAX_CONCURRENT_BLOCKING_OPERATIONS,
        ))
    }));
    super::blocking::run_bounded("http audit", gate, move || emit_http_audit(event)).await
}

#[cfg(test)]
pub(super) fn install_audit_hook(hook: AuditHook) {
    *audit_hook_slot().write().expect("audit hook lock") = Some(hook);
}

#[cfg(test)]
pub(super) fn clear_audit_hook() {
    *audit_hook_slot().write().expect("audit hook lock") = None;
}

pub(super) fn safe_audit_run_id(run_id: &str) -> Option<String> {
    (!run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| run_id.to_string())
}

pub(super) fn mutation_path_run_id(method: &str, path: &str) -> Option<String> {
    if method != "POST" {
        return None;
    }
    let suffix = path.strip_prefix("/v1/runs/")?;
    let (run_id, mutation) = suffix.split_once('/')?;
    matches!(mutation, "cancel" | "dead-letter")
        .then(|| safe_audit_run_id(run_id))
        .flatten()
}
