pub fn intentionally_invalid(value: i32) -> i32 {
    if value > 0 { value } else { 0 }
