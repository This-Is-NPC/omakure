use super::{OperationError, OperationErrorCode};
use std::time::Duration;

const MAX_WAIT_SECONDS: u32 = 600;

pub(super) fn bounded_wait(seconds: u32) -> Duration {
    Duration::from_secs(u64::from(seconds.min(MAX_WAIT_SECONDS)))
}

pub(super) fn no_session_error() -> OperationError {
    OperationError::new(
        OperationErrorCode::NotFound,
        "this node holds no session with that peer",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_wait_is_capped_at_ten_minutes() {
        assert_eq!(bounded_wait(0), Duration::ZERO);
        assert_eq!(bounded_wait(120), Duration::from_secs(120));
        assert_eq!(bounded_wait(u32::MAX), Duration::from_secs(600));
    }
}
