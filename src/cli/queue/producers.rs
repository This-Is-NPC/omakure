use crate::cli::args::{QueueAddArgs, QueueCancelArgs, QueueDeadLetterArgs};
use crate::cli::emit::{default_operation_error_code, emit_error, emit_operation_error};
use crate::cli::json::{self, codes};
use crate::operations::core::{self, CancelRunRequest, DeadLetterRunRequest, EnqueueRunRequest};
use crate::operations::{OperationError, OperationErrorCode};
use crate::workspace::Workspace;
use std::error::Error;

pub(super) fn add(
    workspace: &Workspace,
    opts: QueueAddArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let timeout_ms = match opts.timeout.as_deref() {
        None => None,
        Some(s) => match parse_humantime_duration_ms(s) {
            Ok(ms) => Some(ms),
            Err(err) => return emit_error(json_output, codes::INVALID_ARGUMENT, err.to_string()),
        },
    };

    let row = match core::enqueue_run(
        workspace,
        EnqueueRunRequest {
            script: opts.script,
            args: opts.args,
            env: None,
            secret_fields: Vec::new(),
            run_id: opts.run_id,
            actor: opts.actor,
            reason: opts.reason,
            priority: opts.priority,
            timeout_ms,
            parent_run_id: opts.parent_run_id,
            cron_schedule_id: opts.cron_schedule_id,
        },
    ) {
        Ok(row) => row,
        Err(err) => return emit_operation_error(json_output, err, queue_error_code),
    };
    if json_output {
        json::print_ok(row);
    } else {
        println!("queued: {}", row.run_id);
    }
    Ok(())
}

pub(super) fn cancel(
    workspace: &Workspace,
    opts: QueueCancelArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    match core::cancel_run(
        workspace,
        CancelRunRequest {
            run_id: opts.run_id,
            reason: opts.reason,
        },
    ) {
        Ok(row) => {
            if json_output {
                json::print_ok(row);
            } else {
                println!("cancelled: {}", row.run_id);
            }
            Ok(())
        }
        Err(err) => emit_operation_error(json_output, err, queue_error_code),
    }
}

pub(super) fn dead_letter(
    workspace: &Workspace,
    opts: QueueDeadLetterArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    match core::dead_letter_run(
        workspace,
        DeadLetterRunRequest {
            run_id: opts.run_id,
            reason: opts.reason,
        },
    ) {
        Ok(row) => {
            if json_output {
                json::print_ok(row);
            } else {
                println!("dead_letter: {}", row.run_id);
            }
            Ok(())
        }
        Err(err) => emit_operation_error(json_output, err, queue_error_code),
    }
}

pub(super) fn stats(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let stats = match core::queue_stats(workspace) {
        Ok(s) => s,
        Err(err) => return emit_operation_error(json_output, err, queue_error_code),
    };
    if json_output {
        json::print_ok(stats);
    } else {
        println!("Total: {}", stats.total);
        let mut keys: Vec<_> = stats.counts_by_state.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        for (state, count) in keys {
            println!("  {:<12} {}", state, count);
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(super) enum QueueDurationError {
    #[error("invalid duration `{input}`: {source}")]
    Invalid {
        input: String,
        #[source]
        source: humantime::DurationError,
    },
    #[error("duration too large: {0}")]
    TooLarge(String),
}

pub(super) fn parse_humantime_duration_ms(s: &str) -> Result<i64, QueueDurationError> {
    let trimmed = s.trim();
    let dur = humantime::parse_duration(trimmed).map_err(|source| QueueDurationError::Invalid {
        input: trimmed.to_string(),
        source,
    })?;
    let ms = dur.as_millis();
    if ms > i64::MAX as u128 {
        return Err(QueueDurationError::TooLarge(trimmed.to_string()));
    }
    Ok(ms as i64)
}

fn queue_error_code(err: &OperationError) -> &'static str {
    match err.code {
        OperationErrorCode::UnsafePath | OperationErrorCode::Conflict => codes::INVALID_ARGUMENT,
        _ => default_operation_error_code(err),
    }
}
