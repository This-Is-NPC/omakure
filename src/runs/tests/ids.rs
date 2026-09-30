use super::*;

// -----------------------------------------------------------------
// Misc helpers
// -----------------------------------------------------------------

#[test]
fn generate_run_id_is_monotonic_within_process() {
    let a = generate_run_id();
    let b = generate_run_id();
    let c = generate_run_id();
    assert_ne!(a, b);
    assert_ne!(b, c);
    let counter_of = |s: &str| s.rsplit('-').next().unwrap().parse::<u64>().unwrap();
    assert!(counter_of(&b) > counter_of(&a));
    assert!(counter_of(&c) > counter_of(&b));
}

#[test]
fn format_run_timestamp_known_value() {
    assert_eq!(format_run_timestamp(1705321800000), "2024-01-15 12:30");
}

#[test]
fn format_run_timestamp_zero_and_negative() {
    assert_eq!(format_run_timestamp(0), "1970-01-01 00:00");
    assert_eq!(format_run_timestamp(-1000), "1970-01-01 00:00");
}
