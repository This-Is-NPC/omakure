use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock Unix seconds; 0 when the clock is set before 1970.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Wall-clock Unix milliseconds; 0 when the clock is set before 1970.
pub fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}
