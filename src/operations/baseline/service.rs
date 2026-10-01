use crate::direct_service::BaselineDispatcher;
use crate::node::NodeContext;
use crate::operations::service_delivery::{bounded_wait, no_session_error};
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::util::hex;
use crate::workspace::Workspace;
use serde::Serialize;
use std::time::Duration;

use super::{rollback_baseline, InstalledBaseline};

pub struct BaselineServiceRequest {
    pub peer_node_id: String,
    pub manifest: String,
    pub scripts: Vec<String>,
    pub wait_seconds: u32,
}

pub struct PreparedBaselinePush {
    dispatcher: BaselineDispatcher,
    peer_node_id: String,
    manifest: Vec<u8>,
    scripts: Vec<Vec<u8>>,
    wait: Duration,
}

#[derive(Serialize)]
pub struct BaselineServiceOutcome {
    pushed: bool,
    via: &'static str,
    baseline_id: String,
    answered: bool,
    accepted: bool,
    code: u16,
}

pub fn prepare_service_push(
    dispatcher: Option<BaselineDispatcher>,
    request: BaselineServiceRequest,
) -> OperationResult<PreparedBaselinePush> {
    let dispatcher = dispatcher.ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "no direct transport is running, so there is no session to carry a baseline",
        )
    })?;
    if !dispatcher.has_session(&request.peer_node_id) {
        return Err(no_session_error());
    }
    let (Some(manifest), Some(scripts)) = (
        decode_lower_hex(&request.manifest),
        request
            .scripts
            .iter()
            .map(|body| decode_lower_hex(body))
            .collect::<Option<Vec<_>>>(),
    ) else {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "the manifest and every script body must be lowercase hex",
        ));
    };
    Ok(PreparedBaselinePush {
        dispatcher,
        peer_node_id: request.peer_node_id,
        manifest,
        scripts,
        wait: bounded_wait(request.wait_seconds),
    })
}

pub fn push_prepared_service(
    prepared: PreparedBaselinePush,
) -> OperationResult<BaselineServiceOutcome> {
    let outcome = prepared
        .dispatcher
        .push_baseline(
            &prepared.peer_node_id,
            &prepared.manifest,
            &prepared.scripts,
            prepared.wait,
        )
        .map_err(|error| {
            OperationError::new(
                OperationErrorCode::InvalidInput,
                format!("baseline push failed: {error}"),
            )
        })?;
    Ok(BaselineServiceOutcome {
        pushed: true,
        via: "service",
        baseline_id: outcome.baseline_id,
        answered: outcome.answered,
        accepted: outcome.accepted,
        code: outcome.code,
    })
}

pub fn rollback_local_baseline(
    workspace: &Workspace,
    context: &NodeContext,
    confirmed: bool,
    now: i64,
) -> OperationResult<InstalledBaseline> {
    let policy = crate::baseline_push::read_policy(context);
    rollback_baseline(workspace, &policy, confirmed, now)
}

fn decode_lower_hex(value: &str) -> Option<Vec<u8>> {
    if !hex::is_lower(value) {
        return None;
    }
    hex::decode(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_transport_precedes_payload_decoding() {
        let error = prepare_service_push(
            None,
            BaselineServiceRequest {
                peer_node_id: "peer".into(),
                manifest: "INVALID".into(),
                scripts: vec!["INVALID".into()],
                wait_seconds: 120,
            },
        )
        .err()
        .expect("unavailable transport");
        assert_eq!(error.code, OperationErrorCode::InvalidInput);
        assert_eq!(
            error.message,
            "no direct transport is running, so there is no session to carry a baseline"
        );
    }

    #[test]
    fn lowercase_hex_rejects_uppercase_and_odd_length() {
        assert_eq!(decode_lower_hex("00ff"), Some(vec![0, 255]));
        assert_eq!(decode_lower_hex("00FF"), None);
        assert_eq!(decode_lower_hex("0"), None);
    }
}
