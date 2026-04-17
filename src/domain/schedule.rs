use std::str::FromStr;

use chrono::{DateTime, Utc};
use cron::Schedule as CronSchedule;

use crate::error::SchemaError;

/// Normalize a user-supplied cron expression into the 6-field form
/// expected by the `cron` crate (`sec min hour dom month dow`).
///
/// Supports classic Unix 5-field, 6-field (seconds prefix), and the
/// standard macros. `@reboot` is rejected because it has no calendar
/// semantics the scheduler can evaluate.
pub fn normalize_cron_expr(expr: &str) -> Result<String, SchemaError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(SchemaError::InvalidCron {
            expr: expr.to_string(),
            reason: "expression is empty".to_string(),
        });
    }

    if let Some(macro_expansion) = expand_macro(trimmed) {
        return macro_expansion.map(str::to_string);
    }

    let field_count = trimmed.split_whitespace().count();
    match field_count {
        5 => Ok(format!("0 {}", trimmed)),
        6 | 7 => Ok(trimmed.to_string()),
        other => Err(SchemaError::InvalidCron {
            expr: expr.to_string(),
            reason: format!("expected 5, 6, or 7 fields, got {other}"),
        }),
    }
}

fn expand_macro(expr: &str) -> Option<Result<&'static str, SchemaError>> {
    match expr {
        "@yearly" | "@annually" => Some(Ok("0 0 0 1 1 *")),
        "@monthly" => Some(Ok("0 0 0 1 * *")),
        "@weekly" => Some(Ok("0 0 0 ? * 1")),
        "@daily" | "@midnight" => Some(Ok("0 0 0 * * *")),
        "@hourly" => Some(Ok("0 0 * * * *")),
        "@reboot" => Some(Err(SchemaError::InvalidCron {
            expr: expr.to_string(),
            reason: "@reboot is not supported by the scheduler".to_string(),
        })),
        _ if expr.starts_with('@') => Some(Err(SchemaError::InvalidCron {
            expr: expr.to_string(),
            reason: "unknown cron macro".to_string(),
        })),
        _ => None,
    }
}

/// Parse a user-supplied cron expression into a `CronSchedule`.
pub fn parse_cron(expr: &str) -> Result<CronSchedule, SchemaError> {
    let normalized = normalize_cron_expr(expr)?;
    CronSchedule::from_str(&normalized).map_err(|e| SchemaError::InvalidCron {
        expr: expr.to_string(),
        reason: e.to_string(),
    })
}

/// Compute the next fire time strictly after `after`.
pub fn next_fire_after(schedule: &CronSchedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    schedule.after(&after).next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_classic_5_field() {
        assert!(parse_cron("*/5 * * * *").is_ok());
        assert!(parse_cron("0 9 * * 1-5").is_ok());
    }

    #[test]
    fn parses_6_field_with_seconds() {
        assert!(parse_cron("0 0 12 * * *").is_ok());
    }

    #[test]
    fn parses_macros() {
        for m in [
            "@hourly",
            "@daily",
            "@midnight",
            "@weekly",
            "@monthly",
            "@yearly",
            "@annually",
        ] {
            assert!(parse_cron(m).is_ok(), "macro {m} should parse");
        }
    }

    #[test]
    fn rejects_reboot_macro() {
        let err = parse_cron("@reboot").unwrap_err();
        assert!(matches!(err, SchemaError::InvalidCron { .. }));
    }

    #[test]
    fn rejects_unknown_macro() {
        assert!(matches!(
            parse_cron("@never").unwrap_err(),
            SchemaError::InvalidCron { .. }
        ));
    }

    #[test]
    fn rejects_empty_expr() {
        assert!(matches!(
            parse_cron("   ").unwrap_err(),
            SchemaError::InvalidCron { .. }
        ));
    }

    #[test]
    fn rejects_bad_field_count() {
        assert!(matches!(
            parse_cron("* * *").unwrap_err(),
            SchemaError::InvalidCron { .. }
        ));
    }

    #[test]
    fn rejects_invalid_expression() {
        assert!(matches!(
            parse_cron("99 * * * *").unwrap_err(),
            SchemaError::InvalidCron { .. }
        ));
    }

    #[test]
    fn next_fire_advances() {
        let schedule = parse_cron("0 0 12 * * *").unwrap();
        let anchor = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let next = next_fire_after(&schedule, anchor).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap());
    }

    #[test]
    fn hourly_macro_fires_each_hour() {
        let schedule = parse_cron("@hourly").unwrap();
        let anchor = Utc.with_ymd_and_hms(2026, 1, 1, 0, 30, 0).unwrap();
        let next = next_fire_after(&schedule, anchor).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 1, 0, 0).unwrap());
    }
}
