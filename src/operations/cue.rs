use super::service_delivery::{bounded_wait, no_session_error};
use super::{OperationError, OperationErrorCode, OperationResult};
use crate::direct_service::CueDispatcher;
use serde::Serialize;
use std::time::Duration;

const CUE_ID_INVALID_MESSAGE: &str = "cue id must be 32 lowercase hexadecimal characters";

pub fn validate_cue_id(cue_id: Option<&str>) -> OperationResult<()> {
    if cue_id.is_some_and(|id| !crate::remote_cue::is_well_formed_cue_id(id)) {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            CUE_ID_INVALID_MESSAGE,
        ));
    }
    Ok(())
}

pub struct CueServiceRequest {
    pub peer_node_id: String,
    pub script: String,
    pub reason: String,
    pub wait_seconds: u32,
    pub cue_id: Option<String>,
}

pub struct PreparedCueDispatch {
    dispatcher: CueDispatcher,
    request: CueServiceRequest,
    wait: Duration,
}

#[derive(Serialize)]
pub struct CueServiceOutcome {
    dispatched: bool,
    via: &'static str,
    cue_id: String,
    expected_run_id: String,
    answered: bool,
    accepted: bool,
    code: u16,
    outcome_seen: bool,
}

pub fn prepare_service_dispatch(
    dispatcher: Option<CueDispatcher>,
    request: CueServiceRequest,
) -> OperationResult<PreparedCueDispatch> {
    validate_cue_id(request.cue_id.as_deref())?;
    let dispatcher = dispatcher.ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "no direct transport is running, so there is no session to carry a cue",
        )
    })?;
    if !dispatcher.has_session(&request.peer_node_id) {
        return Err(no_session_error());
    }
    let wait = bounded_wait(request.wait_seconds);
    Ok(PreparedCueDispatch {
        dispatcher,
        request,
        wait,
    })
}

pub fn dispatch_prepared_service(
    prepared: PreparedCueDispatch,
) -> OperationResult<CueServiceOutcome> {
    let request = prepared.request;
    let outcome = prepared
        .dispatcher
        .dispatch(
            &request.peer_node_id,
            &request.script,
            &request.reason,
            prepared.wait,
            request.cue_id.as_deref(),
        )
        .map_err(|error| {
            OperationError::new(
                OperationErrorCode::InvalidInput,
                format!("cue dispatch failed: {error}"),
            )
        })?;
    Ok(CueServiceOutcome {
        dispatched: true,
        via: "service",
        cue_id: outcome.cue_id,
        expected_run_id: outcome.expected_run_id,
        answered: outcome.answered,
        accepted: outcome.accepted,
        code: outcome.code,
        outcome_seen: outcome.outcome_seen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_cue_id_precedes_unavailable_transport() {
        let invalid = prepare_service_dispatch(
            None,
            CueServiceRequest {
                peer_node_id: "peer".into(),
                script: "script".into(),
                reason: "reason".into(),
                wait_seconds: 120,
                cue_id: Some("BAD".into()),
            },
        );
        let error = invalid.err().expect("invalid cue id");
        assert_eq!(error.code, OperationErrorCode::InvalidInput);
        assert_eq!(error.message, CUE_ID_INVALID_MESSAGE);

        let unavailable = prepare_service_dispatch(
            None,
            CueServiceRequest {
                peer_node_id: "peer".into(),
                script: "script".into(),
                reason: "reason".into(),
                wait_seconds: 120,
                cue_id: None,
            },
        );
        let error = unavailable.err().expect("unavailable transport");
        assert_eq!(error.code, OperationErrorCode::InvalidInput);
        assert_eq!(
            error.message,
            "no direct transport is running, so there is no session to carry a cue"
        );
    }
}
