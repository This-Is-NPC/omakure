use std::collections::HashSet as Set;
use std::str::FromStr;

fn parse(value: &str) -> bool {
    Set::<String>::from_iter([value.to_string()]);
    bool::from_str(value).is_ok()
}
