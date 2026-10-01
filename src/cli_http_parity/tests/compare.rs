use super::*;

#[test]
fn comparator_keeps_required_and_security_observables_strict() {
    let schema = comparison_schema();
    let left = serde_json::json!({
        "status": "ok",
        "id": "fixture-cli-id",
        "created_at": "2026-01-01T00:00:00Z",
        "auth": "authorized",
        "redacted": true,
        "items": ["first", "second"]
    });
    let mut right = left.clone();
    right["status"] = serde_json::json!("failed");
    assert!(compare_observables_for_case(&schema, "fixture.success", &left, &right).is_err());
    let mut right = left.clone();
    right["auth"] = serde_json::json!("forbidden");
    assert!(compare_observables_for_case(&schema, "fixture.success", &left, &right).is_err());
    let mut right = left.clone();
    right["items"] = serde_json::json!(["second", "first"]);
    assert!(compare_observables_for_case(&schema, "fixture.success", &left, &right).is_err());
}

#[test]
fn comparator_allows_only_declared_transport_generated_and_time_changes() {
    let schema = comparison_schema();
    let left = serde_json::json!({
        "status": "ok",
        "id": "fixture-cli-id",
        "created_at": "2026-01-01T00:00:00Z",
        "metadata": {"a": 1, "b": 2}
    });
    let right = serde_json::json!({
        "data": {
            "status": "ok",
            "id": "fixture-http-id",
            "created_at": "2027-01-01T00:00:00Z",
            "metadata": {"b": 2, "a": 1}
        }
    });
    assert!(compare_observables_for_case(&schema, "fixture.success", &left, &right).is_ok());
    let mut changed = right.clone();
    changed["data"]["status"] = serde_json::Value::Null;
    assert!(compare_observables_for_case(&schema, "fixture.success", &left, &changed).is_err());
    let mut absent = right["data"].clone();
    absent.as_object_mut().unwrap().remove("status");
    assert!(compare_observables_for_case(&schema, "fixture.success", &left, &absent).is_err());
}

#[test]
fn comparator_rejects_false_boolean_invariants() {
    let mut schema = comparison_schema();
    schema.case_requirements[0].invariant_fields = vec!["status".into()];
    let value = serde_json::json!({
        "status": false,
        "id": "fixture-id",
        "created_at": "2026-01-01T00:00:00Z"
    });
    assert!(matches!(
        compare_observables_for_case(&schema, "fixture.success", &value, &value),
        Err(ObservableError::InvalidInvariant { .. })
    ));
}

#[test]
fn generated_normalization_does_not_rewrite_undeclared_identity_paths() {
    let schema = comparison_schema();
    let left = serde_json::json!({
        "status": "ok",
        "id": "generated-cli",
        "caller_id": "caller-a",
        "created_at": "2026-01-01T00:00:00Z"
    });
    let right = serde_json::json!({
        "status": "ok",
        "id": "generated-http",
        "caller_id": "caller-b",
        "created_at": "2026-01-01T00:00:00Z"
    });
    assert!(compare_observables_for_case(&schema, "fixture.success", &left, &right).is_err());
}

#[test]
fn comparator_reports_missing_case_and_envelope_variants() {
    let mut schema = comparison_schema();
    schema.case_requirements.clear();
    assert!(matches!(
        compare_observables_for_case(
            &schema,
            "fixture.success",
            &serde_json::json!({"status": "ok"}),
            &serde_json::json!({"status": "ok"})
        ),
        Err(ObservableError::MissingCaseRequirement { .. })
    ));

    for envelope in ["result", "response", "body"] {
        let wrapped =
            serde_json::json!({envelope: {"status": "ok", "id": "same", "created_at": "now"}});
        let mut schema = comparison_schema();
        schema
            .allowed_normalizations
            .retain(|rule| *rule != NormalizationRule::GeneratedId);
        assert!(
            compare_observables_for_case(&schema, "fixture.success", &wrapped, &wrapped).is_ok()
        );
    }
    let schema = comparison_schema();
    let value = serde_json::json!({"status": "ok", "id": "same", "created_at": "now"});
    assert!(
        compare_observables_for_case(
            &schema,
            "fixture.success",
            &value,
            &serde_json::json!({"transport": true})
        )
        .is_err()
    );
}
