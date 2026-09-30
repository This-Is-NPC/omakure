use crate::util::time::unix_millis;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------------

/// Generate a synthetic, sortable run id of the form
/// `<unix_ms>-<pid>-<counter>`.
///
/// The counter is process-local and monotonic so two ids generated within
/// the same millisecond by the same process never collide. Across
/// processes, the `<pid>` segment provides uniqueness.
pub fn generate_run_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ms = unix_millis();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{}", ms, std::process::id(), counter)
}

/// Format a Unix-millisecond timestamp as `YYYY-MM-DD HH:MM` (UTC).
/// Used by history CLI and API consumers.
pub fn format_run_timestamp(timestamp_ms: i64) -> String {
    let mut ms = timestamp_ms;
    if ms < 0 {
        ms = 0;
    }
    let seconds = ms / 1000;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;

    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        year, month, day, hour, minute
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}
