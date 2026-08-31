//! Fixed corpus: async, closures, matches, ?, nesting, and logging macros.

#[allow(dead_code)]
pub async fn async_pipeline(input: Option<Result<i32, &'static str>>) -> Result<i32, &'static str> {
    let transform = |value| if value > 2 { value * 2 } else { value + 1 };
    let value = input.transpose()?.unwrap_or_default();
    match value {
        0 => Ok(transform(value)),
        1..=4 => Ok(transform(value)),
        _ if value > 8 => Err("large"),
        _ => Ok(value),
    }
}

pub fn nested_tables(items: &[Option<i32>]) -> i32 {
    let mut total = 0;
    for item in items {
        if let Some(value) = item {
            total += match value {
                0 => 0,
                1 | 2 => *value,
                _ => *value + 1,
            };
        }
    }
    total
}

macro_rules! logged { ($expr:expr) => {{ eprintln!("value={:?}", $expr); $expr }} }

pub fn macro_wrapper(value: i32) -> i32 {
    logged!(value)
}
