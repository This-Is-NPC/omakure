use crate::operations::{OperationError, OperationErrorCode, OperationResult};

pub(super) fn query_pairs(raw_query: Option<&str>) -> OperationResult<Vec<(String, String)>> {
    match raw_query {
        Some(query) => serde_urlencoded::from_str(query).map_err(|err| {
            OperationError::new(
                OperationErrorCode::InvalidInput,
                format!("invalid query string: {err}"),
            )
        }),
        None => Ok(Vec::new()),
    }
}

pub(super) fn query_value(pairs: &[(String, String)], key: &str) -> Option<String> {
    query_values(pairs, key).into_iter().next()
}

pub(super) fn query_values(pairs: &[(String, String)], key: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.clone())
        .collect()
}

pub(super) fn query_i64(pairs: &[(String, String)], key: &str) -> OperationResult<Option<i64>> {
    query_value(pairs, key)
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    format!("invalid integer query parameter: {key}"),
                )
            })
        })
        .transpose()
}

pub(super) fn query_bool(pairs: &[(String, String)], key: &str) -> OperationResult<Option<bool>> {
    query_value(pairs, key)
        .map(|value| {
            value.parse::<bool>().map_err(|_| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    format!("invalid boolean query parameter: {key}"),
                )
            })
        })
        .transpose()
}
