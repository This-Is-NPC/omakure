use super::model::{
    Manifest, ManifestError, NormalizationRule, ObservableActor, ObservableCaseRequirement,
    ObservableRule, ObservableSchema, ParityClass,
};
use super::{checked_manifest, SCHEMA_VERSION};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorCase {
    pub behavior_case: String,
    pub entry_id: String,
    pub operation_family: String,
    pub class: ParityClass,
    pub cli_ids: Vec<String>,
    pub http_ids: Vec<String>,
}

/// Parse and validate the checked-in schemas and cases.
pub fn checked_registry() -> Result<(Manifest, Vec<BehaviorCase>), ManifestError> {
    let manifest = checked_manifest()?;
    manifest.validate_observable_registry()?;
    let cases = manifest.behavior_cases()?;
    Ok((manifest, cases))
}

pub(super) fn validate_observable_schema(schema: &ObservableSchema) -> Result<(), ManifestError> {
    let family = schema.operation_family.trim();
    validate_schema_header(schema, family)?;
    validate_schema_normalizations(schema)?;
    validate_schema_metadata(schema, family)?;
    validate_case_requirements(schema, family)
}

fn validate_schema_header(schema: &ObservableSchema, family: &str) -> Result<(), ManifestError> {
    if family.is_empty() {
        return Err(invalid_schema(schema, "operation_family is empty"));
    }
    if schema.version != SCHEMA_VERSION {
        return Err(invalid_schema(
            schema,
            &format!("unsupported version {}", schema.version),
        ));
    }
    validate_semantic_fields(family, "required_fields", &schema.required_fields)
}

fn validate_schema_normalizations(schema: &ObservableSchema) -> Result<(), ManifestError> {
    let mut normalizations = BTreeSet::new();
    if schema
        .allowed_normalizations
        .iter()
        .any(|normalization| !normalizations.insert(*normalization))
    {
        return Err(invalid_schema(
            schema,
            "allowed_normalizations must be unique",
        ));
    }
    let has_timestamps = !schema.nondeterministic_fields.is_empty();
    let allows_timestamps = schema
        .allowed_normalizations
        .contains(&NormalizationRule::NondeterministicTimestamp);
    if has_timestamps != allows_timestamps {
        return Err(invalid_schema(
            schema,
            "nondeterministic_fields must match timestamp normalization",
        ));
    }
    if schema.success_cases.is_empty() || schema.error_cases.is_empty() {
        return Err(invalid_schema(
            schema,
            "success_cases and error_cases must be nonempty",
        ));
    }
    Ok(())
}

fn validate_schema_metadata(schema: &ObservableSchema, family: &str) -> Result<(), ManifestError> {
    if schema
        .ignored_fields
        .iter()
        .any(|field| !field.trim().to_ascii_lowercase().starts_with("transport."))
    {
        return Err(invalid_schema(
            schema,
            "only transport fields may be ignored",
        ));
    }
    let actors_valid = [
        ObservableActor::Authorized,
        ObservableActor::Unauthenticated,
        ObservableActor::Forbidden,
    ]
    .iter()
    .all(|actor| schema.actors.contains(actor));
    if schema.actors.is_empty() || !actors_valid {
        return Err(invalid_schema(
            schema,
            "actors must include authorized, unauthenticated and forbidden",
        ));
    }
    validate_observable_rules(schema, family)
}

fn validate_observable_rules(schema: &ObservableSchema, family: &str) -> Result<(), ManifestError> {
    let forbidden = ["auth", "errors", "redaction", "state", "retry"];
    let rules = [
        ("ordering", schema.ordering),
        ("pagination", schema.pagination),
        ("time", schema.time),
        ("auth", schema.auth),
        ("errors", schema.errors),
        ("redaction", schema.redaction),
        ("state", schema.state),
        ("retry", schema.retry),
    ];
    if let Some((name, _)) = rules
        .iter()
        .find(|(name, rule)| forbidden.contains(name) && *rule == ObservableRule::NotApplicable)
    {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{name} cannot be not-applicable"),
        });
    }
    Ok(())
}

fn validate_case_requirements(
    schema: &ObservableSchema,
    family: &str,
) -> Result<(), ManifestError> {
    if schema.case_requirements.is_empty() {
        return Err(invalid_schema(schema, "case_requirements must be nonempty"));
    }
    let mut cases = BTreeSet::new();
    for requirement in &schema.case_requirements {
        validate_case_requirement(requirement, family, &mut cases)?;
    }
    Ok(())
}

fn validate_case_requirement(
    requirement: &ObservableCaseRequirement,
    family: &str,
    cases: &mut BTreeSet<String>,
) -> Result<(), ManifestError> {
    if requirement.behavior_case.trim().is_empty()
        || !cases.insert(requirement.behavior_case.clone())
    {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: "case_requirements must have unique nonempty behavior_case values".into(),
        });
    }
    validate_semantic_fields(family, "case required_fields", &requirement.required_fields)?;
    validate_case_paths(
        requirement,
        family,
        &requirement.invariant_fields,
        "invariant",
    )?;
    validate_case_paths(
        requirement,
        family,
        &requirement.generated_id_fields,
        "generated ID",
    )
}

fn validate_case_paths(
    requirement: &ObservableCaseRequirement,
    family: &str,
    paths: &[String],
    label: &str,
) -> Result<(), ManifestError> {
    if let Some(path) = paths.iter().find(|path| {
        !requirement
            .required_fields
            .iter()
            .any(|field| field == *path)
    }) {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{label} {path} is not required"),
        });
    }
    Ok(())
}

fn invalid_schema(schema: &ObservableSchema, reason: &str) -> ManifestError {
    ManifestError::InvalidObservableSchema {
        family: schema.operation_family.clone(),
        reason: reason.into(),
    }
}

fn validate_semantic_fields(
    family: &str,
    label: &str,
    fields: &[String],
) -> Result<(), ManifestError> {
    if fields.is_empty() || fields.iter().any(|field| field.trim().is_empty()) {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{label} must be nonempty"),
        });
    }
    let mut seen = BTreeSet::new();
    let all_envelopes = fields.iter().all(|field| {
        matches!(
            field.trim().to_ascii_lowercase().as_str(),
            "ok" | "data" | "result" | "response" | "body"
        )
    });
    let has_duplicate = fields
        .iter()
        .map(|field| field.trim().to_ascii_lowercase())
        .any(|field| !seen.insert(field));
    if all_envelopes || has_duplicate {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{label} must contain unique semantic paths"),
        });
    }
    Ok(())
}
