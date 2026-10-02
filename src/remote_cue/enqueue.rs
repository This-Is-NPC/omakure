use super::codes::CueCode;
use super::session::CueSession;
use crate::util::hex;

/// The result of trying to turn an accepted Cue into a durable run.
///
/// Only the run-id uniqueness constraint proves that this Cue already has a
/// durable run. Every other failure is retained as a stable local operation
/// error and fails closed without an acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueEnqueueError {
    NoWorkspace,
    Duplicate,
    Failed(crate::operations::OperationErrorCode),
}

impl CueEnqueueError {
    pub(crate) fn stable_name(&self) -> &'static str {
        match self {
            Self::NoWorkspace => CueCode::ScriptUnresolvable.name(),
            Self::Duplicate => CueCode::Duplicate.name(),
            Self::Failed(code) => code.as_str(),
        }
    }
}

impl std::fmt::Display for CueEnqueueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.stable_name())
    }
}

impl<'a> CueSession<'a> {
    /// Turn an accepted decision into one run.
    ///
    /// Called after `handle_envelope` authorizes the script and verifies its
    /// content hash at the accept transition.
    ///
    /// The run id is supplied by the caller and derived from the cue id, so the
    /// primary key refuses a second insert. Only that uniqueness failure is a
    /// duplicate; all other operation failures remain visible and fail closed.
    pub fn enqueue_accepted(
        &self,
        cue_id: &str,
        script: &str,
        reason: &str,
        authorized_content_hash: &str,
    ) -> Result<String, CueEnqueueError> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or(CueEnqueueError::NoWorkspace)?;
        let run_id = derive_run_id(cue_id);
        crate::operations::core::enqueue_cue_run(
            workspace,
            crate::operations::core::EnqueueRunRequest {
                script: script.to_string(),
                args: Vec::new(),
                env: None,
                secret_fields: Vec::new(),
                run_id: Some(run_id.clone()),
                actor: self.remote_node_id.clone(),
                reason: Some(reason.to_string()),
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
            },
            authorized_content_hash,
        )
        .map(|_| run_id)
        .map_err(classify_enqueue_error)
    }
}

pub(super) fn classify_enqueue_error(error: crate::operations::OperationError) -> CueEnqueueError {
    if error.code == crate::operations::OperationErrorCode::IoFailed
        && error
            .message
            .ends_with("UNIQUE constraint failed: runs.run_id")
    {
        CueEnqueueError::Duplicate
    } else {
        CueEnqueueError::Failed(error.code)
    }
}

/// The local run id for a Cue, under its own domain separator.
///
/// Deterministic so the Conductor can compute the opaque run id it will see on
/// the `run-completed` Signal without any message carrying a correlation field,
/// and so the database primary key is the durable at-most-once guard.
///
/// The domain separator is what stops a cue id being replayable as a preimage
/// in any other construction that hashes ids.
pub fn derive_run_id(cue_id: &str) -> String {
    hex::encode(&crate::util::digest::sha256_domain(
        b"omakure/cue-run-id/v1\0",
        cue_id.as_bytes(),
    ))
}
