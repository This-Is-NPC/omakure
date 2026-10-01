use crate::operations::{OperationError, OperationErrorCode};
use std::sync::Arc;

pub(super) async fn run_bounded<T: Send + 'static>(
    operation: &'static str,
    gate: Arc<tokio::sync::Semaphore>,
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, OperationError> {
    run_bounded_with_join(operation, gate, task, |_| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("{operation} operation failed"),
        )
    })
    .await
}

pub(super) async fn run_bounded_with_join<T: Send + 'static>(
    operation: &'static str,
    gate: Arc<tokio::sync::Semaphore>,
    task: impl FnOnce() -> T + Send + 'static,
    join_error: impl FnOnce(tokio::task::JoinError) -> OperationError,
) -> Result<T, OperationError> {
    let permit = gate.acquire_owned().await.map_err(|_| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("{operation} operation unavailable"),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        task()
    })
    .await
    .map_err(join_error)
}
