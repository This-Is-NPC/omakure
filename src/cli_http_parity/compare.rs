use super::model::{NormalizationRule, ObservableSchema};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservableError {
    MissingField { side: &'static str, field: String },
    MissingCaseRequirement { case: String },
    InvalidInvariant { side: &'static str, field: String },
    Mismatch { field: String },
}

impl fmt::Display for ObservableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ObservableError {}

/// Compare observations using the exact semantic requirement for one case.
pub fn compare_observables_for_case(
    schema: &ObservableSchema,
    behavior_case: &str,
    cli: &serde_json::Value,
    http: &serde_json::Value,
) -> Result<(), ObservableError> {
    let (left, right) = validated_observations(schema, behavior_case, cli, http)?;
    if left != right {
        return Err(ObservableError::Mismatch {
            field: "<observable>".into(),
        });
    }
    Ok(())
}

/// Validate each side of a semantic-mismatch case without requiring equality.
pub fn validate_observables_for_case(
    schema: &ObservableSchema,
    behavior_case: &str,
    cli: &serde_json::Value,
    http: &serde_json::Value,
) -> Result<(), ObservableError> {
    validated_observations(schema, behavior_case, cli, http).map(|_| ())
}

fn validated_observations(
    schema: &ObservableSchema,
    behavior_case: &str,
    cli: &serde_json::Value,
    http: &serde_json::Value,
) -> Result<(serde_json::Value, serde_json::Value), ObservableError> {
    let requirement = schema
        .case_requirements
        .iter()
        .find(|requirement| requirement.behavior_case == behavior_case)
        .ok_or_else(|| ObservableError::MissingCaseRequirement {
            case: behavior_case.into(),
        })?;
    let required = &requirement.required_fields;
    let generated = &requirement.generated_id_fields;
    let invariants = &requirement.invariant_fields;
    let left = normalized_observation(schema, cli, required, generated, "cli")?;
    let right = normalized_observation(schema, http, required, generated, "http")?;
    validate_invariants(&left, invariants, "cli")?;
    validate_invariants(&right, invariants, "http")?;
    Ok((left, right))
}

fn normalized_observation(
    schema: &ObservableSchema,
    original: &serde_json::Value,
    required: &[String],
    generated: &[String],
    side: &'static str,
) -> Result<serde_json::Value, ObservableError> {
    let normalize_envelope = schema
        .allowed_normalizations
        .contains(&NormalizationRule::Envelope);
    let mut value = if normalize_envelope {
        strip_transport_envelope(original.clone())
    } else {
        original.clone()
    };
    for field in required {
        if lookup_observable(&value, field).is_none()
            && !(normalize_envelope && lookup_observable(original, field).is_some())
        {
            return Err(ObservableError::MissingField {
                side,
                field: field.clone(),
            });
        }
    }
    let timestamps: BTreeSet<_> = schema
        .nondeterministic_fields
        .iter()
        .map(String::as_str)
        .collect();
    let generated: BTreeSet<_> = if schema
        .allowed_normalizations
        .contains(&NormalizationRule::GeneratedId)
    {
        generated.iter().map(String::as_str).collect()
    } else {
        BTreeSet::new()
    };
    let mut ids = BTreeMap::new();
    canonicalize_observation(&mut value, "", &generated, &timestamps, &mut ids);
    Ok(value)
}

fn validate_invariants(
    value: &serde_json::Value,
    invariants: &[String],
    side: &'static str,
) -> Result<(), ObservableError> {
    for field in invariants {
        if lookup_observable(value, field) != Some(&serde_json::Value::Bool(true)) {
            return Err(ObservableError::InvalidInvariant {
                side,
                field: field.clone(),
            });
        }
    }
    Ok(())
}

fn lookup_observable<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn strip_transport_envelope(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(mut object) if object.len() == 1 => {
            for key in ["data", "result", "response", "body"] {
                if let Some(inner) = object.remove(key) {
                    return strip_transport_envelope(inner);
                }
            }
            serde_json::Value::Object(object)
        }
        other => other,
    }
}

struct Canonicalizer<'a> {
    generated: &'a BTreeSet<&'a str>,
    timestamps: &'a BTreeSet<&'a str>,
    ids: BTreeMap<String, String>,
}

impl<'a> Canonicalizer<'a> {
    fn visit(&mut self, value: &mut serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Object(object) => {
                for (key, child) in object {
                    let child_path = join_path(path, key);
                    self.visit(child, &child_path);
                }
            }
            serde_json::Value::Array(array) => {
                for (index, child) in array.iter_mut().enumerate() {
                    let child_path = join_path(path, &index.to_string());
                    self.visit(child, &child_path);
                }
            }
            serde_json::Value::String(text) => self.normalize_string(text, path),
            _ => {}
        }
    }

    fn normalize_string(&mut self, text: &mut String, path: &str) {
        if path_matches(self.timestamps, path) {
            *text = "<nondeterministic-timestamp>".into();
        } else if path_matches(self.generated, path) {
            let original = text.clone();
            let ordinal = self.ids.len() + 1;
            let canonical = self
                .ids
                .entry(original)
                .or_insert_with(|| format!("<generated-id-{ordinal}>"))
                .clone();
            *text = canonical;
        }
    }
}

fn canonicalize_observation(
    value: &mut serde_json::Value,
    path: &str,
    generated: &BTreeSet<&str>,
    timestamps: &BTreeSet<&str>,
    ids: &mut BTreeMap<String, String>,
) {
    let mut canonicalizer = Canonicalizer {
        generated,
        timestamps,
        ids: std::mem::take(ids),
    };
    canonicalizer.visit(value, path);
    *ids = canonicalizer.ids;
}

fn join_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.into()
    } else {
        format!("{parent}.{child}")
    }
}

fn path_matches(patterns: &BTreeSet<&str>, path: &str) -> bool {
    patterns.iter().any(|pattern| {
        let expected: Vec<_> = pattern.split('.').collect();
        let actual: Vec<_> = path.split('.').collect();
        expected.len() == actual.len()
            && expected
                .iter()
                .zip(actual)
                .all(|(expected, actual)| *expected == "*" || *expected == actual)
    })
}
