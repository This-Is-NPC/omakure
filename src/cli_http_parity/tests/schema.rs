use super::super::schema::validate_observable_schema;
use super::*;

#[test]
fn registry_reports_manifest_case_and_family_counts() {
    let (manifest, cases) = checked_registry().unwrap();
    assert_eq!(
        manifest
            .entries
            .iter()
            .filter(|entry| matches!(
                entry.class,
                ParityClass::Exact | ParityClass::SemanticMismatch
            ))
            .count(),
        cases.len()
    );
    assert_eq!(cases.len(), 44);
    assert_eq!(manifest.schemas.len(), 14);
}

#[test]
fn schema_rejects_empty_required_fields_and_missing_actor() {
    let mut schema = comparison_schema();
    schema.required_fields.clear();
    assert!(validate_observable_schema(&schema).is_err());
    let mut schema = comparison_schema();
    schema.actors.pop();
    assert!(validate_observable_schema(&schema).is_err());
    let mut schema = comparison_schema();
    schema.ignored_fields = vec!["auth.decision".into()];
    assert!(validate_observable_schema(&schema).is_err());
    let mut schema = comparison_schema();
    schema.required_fields = vec!["ok".into()];
    assert!(validate_observable_schema(&schema).is_err());
}

#[test]
fn schema_validation_rejects_each_unsafe_metadata_shape() {
    let mut schema = comparison_schema();
    schema.operation_family.clear();
    assert!(matches!(
        validate_observable_schema(&schema),
        Err(ManifestError::InvalidObservableSchema { .. })
    ));

    let mut schema = comparison_schema();
    schema.version = 2;
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema
        .allowed_normalizations
        .push(NormalizationRule::Envelope);
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.nondeterministic_fields.clear();
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema
        .allowed_normalizations
        .retain(|rule| *rule != NormalizationRule::NondeterministicTimestamp);
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.success_cases.clear();
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.ignored_fields = vec!["auth.decision".into()];
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.actors.pop();
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.auth = ObservableRule::NotApplicable;
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.case_requirements.clear();
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.case_requirements[0].behavior_case = " ".into();
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema
        .case_requirements
        .push(schema.case_requirements[0].clone());
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.required_fields = vec![" ".into()];
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.required_fields = vec!["ok".into()];
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.required_fields = vec!["status".into(), "STATUS".into()];
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.case_requirements[0].invariant_fields = vec!["missing".into()];
    assert!(validate_observable_schema(&schema).is_err());

    let mut schema = comparison_schema();
    schema.case_requirements[0].generated_id_fields = vec!["missing".into()];
    assert!(validate_observable_schema(&schema).is_err());
}

#[test]
fn registry_validation_rejects_missing_or_orphaned_case_metadata() {
    let mut manifest = valid();
    manifest.schemas.clear();
    assert!(matches!(
        manifest.validate_observable_registry(),
        Err(ManifestError::MissingObservableSchema { .. })
    ));

    let mut manifest = valid();
    let mut unknown_schema = manifest.schemas[0].clone();
    unknown_schema.operation_family = "unknown".into();
    manifest.schemas.push(unknown_schema);
    assert!(matches!(
        manifest.validate_observable_registry(),
        Err(ManifestError::UnknownObservableSchema(_))
    ));

    let mut manifest = valid();
    manifest.entries[0].behavior_case = None;
    assert!(matches!(
        manifest.validate_observable_registry(),
        Err(ManifestError::MissingBehaviorCase(_))
    ));

    let mut manifest = valid();
    manifest.schemas[0].case_requirements[0].behavior_case = "orphan".into();
    assert!(matches!(
        manifest.validate_observable_registry(),
        Err(ManifestError::InvalidObservableSchema { .. })
    ));
}
