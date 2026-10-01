use crate::util::sqlite::is_lock_contention;

#[derive(Debug, thiserror::Error)]
pub enum RunsError {
    #[error("not_found: {0}")]
    NotFound(String),
    #[error("{0}")]
    InvalidEnqueue(&'static str),
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
