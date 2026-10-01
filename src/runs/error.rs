use super::state::RunState;
use crate::util::sqlite::WalOpenError;
use crate::util::sqlite::is_lock_contention;

#[derive(Debug, thiserror::Error)]
pub enum RunsError {
    #[error("not_found: {0}")]
    NotFound(String),
    #[error("{0}")]
    InvalidEnqueue(&'static str),
    #[error("run not found: {0}")]
    RunNotFound(String),
    #[error("run not found after cancel: {0}")]
    RunNotFoundAfterCancel(String),
    #[error("run not found after dead_letter: {0}")]
    RunNotFoundAfterDeadLetter(String),
    #[error("illegal transition: cannot move {from} -> {to}; row must be in 'running'")]
    IllegalTransition { from: RunState, to: RunState },
    #[error("cannot cancel run in terminal state '{0}'")]
    TerminalState(RunState),
    #[error(
        "cannot promote run in state '{0}' to dead_letter; only failed or timed_out rows are eligible"
    )]
    DeadLetterIneligible(RunState),
    #[error(transparent)]
    DatabaseOpen(WalOpenError),
    #[error("{operation}: {source}")]
    Filesystem {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{operation}: {source}")]
    Sqlite {
        operation: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

impl RunsError {
    pub(super) fn is_retryable(&self) -> bool {
        matches!(self, Self::Sqlite { source, .. } if is_lock_contention(source))
    }
}
