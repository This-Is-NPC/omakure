use super::*;
use std::collections::BTreeSet;

mod compare;
mod schema;

fn inventory() -> (Vec<String>, Vec<String>) {
    (
        vec!["run".to_string(), "config".to_string()],
        vec!["GET /v1/config".to_string()],
    )
}

fn valid() -> Manifest {
    Manifest {
        schema_version: 1,
        schemas: vec![ObservableSchema {
            operation_family: "config".into(),
            version: 1,
            required_fields: vec!["status".into()],
            ignored_fields: vec![],
            allowed_normalizations: vec![],
            ordering: ObservableRule::Strict,
            pagination: ObservableRule::Strict,
            nondeterministic_fields: vec![],
            time: ObservableRule::Strict,
            auth: ObservableRule::Strict,
            errors: ObservableRule::Strict,
            redaction: ObservableRule::Strict,
            state: ObservableRule::Strict,
            retry: ObservableRule::Strict,
            success_cases: vec!["config.read".into()],
            error_cases: vec!["config.error".into()],
            actors: vec![
                ObservableActor::Authorized,
                ObservableActor::Unauthenticated,
                ObservableActor::Forbidden,
            ],
            case_requirements: vec![ObservableCaseRequirement {
                behavior_case: "config.read".into(),
                required_fields: vec!["status".into()],
                generated_id_fields: vec![],
                invariant_fields: vec![],
            }],
        }],
        entries: vec![
            ParityEntry {
                entry_id: "config".into(),
                class: ParityClass::Exact,
                operation_family: "config".into(),
                behavior_case: Some("config.read".into()),
                docs_anchor: "config".into(),
                cli_ids: vec!["config".into()],
                http_ids: vec!["GET /v1/config".into()],
                rationale: None,
                semantic_difference: None,
                adapter_only: None,
            },
            ParityEntry {
                entry_id: "run-cli".into(),
                class: ParityClass::CliOnly,
                operation_family: "run".into(),
                behavior_case: None,
                docs_anchor: "run-cli".into(),
                cli_ids: vec!["run".into()],
                http_ids: vec![],
                rationale: Some("Inline execution remains local.".into()),
                semantic_difference: None,
                adapter_only: Some(AdapterOnlyRationale {
                    auth: "No HTTP auth surface.".into(),
                    lifecycle: "Runs in the invoking process.".into(),
                }),
            },
        ],
    }
}

#[test]
fn validates_set_equality() {
    let manifest = valid();
    let (cli_ids, http_ids) = inventory();
    manifest
        .validate(SurfaceInventory {
            cli_ids: &cli_ids,
            http_ids: &http_ids,
        })
        .unwrap();
}

#[test]
fn rejects_wildcards() {
    let mut manifest = valid();
    manifest.entries[0].http_ids[0] = "GET /v1/tree/*path".into();
    let (cli_ids, http_ids) = inventory();
    assert!(matches!(
        manifest.validate(SurfaceInventory {
            cli_ids: &cli_ids,
            http_ids: &http_ids
        }),
        Err(ManifestError::WildcardSurface { .. })
    ));
}

#[test]
fn generated_docs_are_deterministic() {
    let manifest = valid();
    assert_eq!(render_markdown(&manifest), render_markdown(&manifest));
    check_docs_freshness(&manifest, &render_markdown(&manifest)).unwrap();
}
#[test]
fn checked_manifest_is_exhaustive() {
    let manifest = super::checked_manifest().unwrap();
    let cli_ids = super::current_cli_ids();
    let http_ids = super::current_http_ids();
    manifest
        .validate(SurfaceInventory {
            cli_ids: &cli_ids,
            http_ids: &http_ids,
        })
        .unwrap();
    assert_eq!(cli_ids.len(), 65);
    assert_eq!(http_ids.len(), 53);
}

#[test]
fn freshness_rejects_changed_docs() {
    let manifest = super::checked_manifest().unwrap();
    assert!(super::check_docs_freshness(&manifest, "stale").is_err());
}
#[test]
fn freshness_accepts_checkout_crlf_without_masking_content_changes() {
    let manifest = super::checked_manifest().unwrap();
    let generated = super::render_markdown(&manifest);
    let crlf = generated.replace('\n', "\r\n");

    super::check_docs_freshness(&manifest, &crlf).unwrap();
    assert!(super::check_docs_freshness(&manifest, &format!("{crlf}drift")).is_err());
}

#[test]
fn rejects_wrong_class_side() {
    let mut manifest = valid();
    manifest.entries[0].class = ParityClass::CliOnly;
    let (cli_ids, http_ids) = inventory();
    assert!(matches!(
        manifest.validate(SurfaceInventory {
            cli_ids: &cli_ids,
            http_ids: &http_ids
        }),
        Err(ManifestError::WrongClassSides { .. })
    ));
}

#[test]
fn rejects_duplicate_surfaces() {
    let mut manifest = valid();
    manifest.entries[1].cli_ids.push("config".into());
    let (cli_ids, http_ids) = inventory();
    assert!(matches!(
        manifest.validate(SurfaceInventory {
            cli_ids: &cli_ids,
            http_ids: &http_ids
        }),
        Err(ManifestError::DuplicateSurface { .. })
    ));
}

#[test]
fn rejects_duplicate_docs_anchors() {
    let mut manifest = valid();
    manifest.entries[1].docs_anchor = "#config".into();
    let (cli_ids, http_ids) = inventory();
    assert!(matches!(
        manifest.validate(SurfaceInventory { cli_ids: &cli_ids, http_ids: &http_ids }),
        Err(ManifestError::DuplicateAnchor(anchor)) if anchor == "config"
    ));
}

#[test]
fn current_inventories_have_expected_size() {
    // Keep the source and router inventories observable to focused tests.
    assert_eq!(super::current_cli_ids().len(), 65);
    assert_eq!(super::current_http_ids().len(), 53);
}
#[test]
fn checked_document_is_fresh_and_anchored() {
    let manifest = super::checked_manifest().unwrap();
    super::check_docs_freshness(&manifest, include_str!("../../../docs/cli-http-parity.md"))
        .unwrap();
}
#[test]
fn invalid_fixtures_are_rejected() {
    let cli_ids = vec!["run".to_string(), "config".to_string()];
    let http_ids = vec!["GET /v1/health".to_string()];
    let inventory = SurfaceInventory {
        cli_ids: &cli_ids,
        http_ids: &http_ids,
    };
    for (name, source) in [
        (
            "duplicate-entry",
            include_str!("../../../fixtures/cli-http-parity/duplicate-entry.toml"),
        ),
        (
            "duplicate-surface",
            include_str!("../../../fixtures/cli-http-parity/duplicate-surface.toml"),
        ),
        (
            "unknown-surface",
            include_str!("../../../fixtures/cli-http-parity/unknown-surface.toml"),
        ),
        (
            "empty-side",
            include_str!("../../../fixtures/cli-http-parity/empty-side.toml"),
        ),
        (
            "wildcard",
            include_str!("../../../fixtures/cli-http-parity/wildcard.toml"),
        ),
        (
            "wrong-class",
            include_str!("../../../fixtures/cli-http-parity/wrong-class.toml"),
        ),
        (
            "duplicate-anchor",
            include_str!("../../../fixtures/cli-http-parity/duplicate-anchor.toml"),
        ),
        (
            "incompatible-semantic",
            include_str!("../../../fixtures/cli-http-parity/incompatible-semantic.toml"),
        ),
        (
            "incompatible-adapter",
            include_str!("../../../fixtures/cli-http-parity/incompatible-adapter.toml"),
        ),
        (
            "unsupported-version",
            include_str!("../../../fixtures/cli-http-parity/unsupported-version.toml"),
        ),
    ] {
        let manifest = Manifest::parse_toml(source).unwrap();
        assert!(
            manifest.validate(inventory.clone()).is_err(),
            "{name} unexpectedly accepted"
        );
    }
}

#[test]
fn all_named_semantic_mismatches_are_present() {
    let manifest = super::checked_manifest().unwrap();
    let kinds: BTreeSet<_> = manifest
        .entries
        .iter()
        .filter_map(|entry| {
            entry
                .semantic_difference
                .as_ref()
                .map(|difference| difference.kind.as_str())
        })
        .collect();
    assert_eq!(kinds.len(), 7);
    for kind in [
        "config-redaction",
        "search-input-limits",
        "battery-https-policy",
        "discovery-snapshot",
        "cue-session",
        "enroll-stage-dial",
        "enroll-token-source",
    ] {
        assert!(kinds.contains(kind), "missing semantic mismatch {kind}");
    }
    assert_eq!(
        manifest
            .entries
            .iter()
            .filter(|entry| entry.class == ParityClass::SemanticMismatch)
            .count(),
        9
    );
}

#[test]
fn every_semantic_mismatch_has_a_paired_case_assertion() {
    let manifest = checked_manifest().unwrap();
    let expected = [
        "mismatch.config",
        "mismatch.search",
        "mismatch.battery-add",
        "mismatch.battery-sync",
        "mismatch.battery-install",
        "mismatch.node-discovery",
        "mismatch.node-cue",
        "mismatch.node-enroll-request",
        "mismatch.node-enroll-apply",
    ];
    for behavior_case in expected {
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.behavior_case.as_deref() == Some(behavior_case))
            .unwrap_or_else(|| panic!("missing mismatch case {behavior_case}"));
        assert_eq!(entry.class, ParityClass::SemanticMismatch);
        assert!(
            !entry.cli_ids.is_empty(),
            "{behavior_case} has no CLI probe"
        );
        assert!(
            !entry.http_ids.is_empty(),
            "{behavior_case} has no HTTP probe"
        );
        let difference = entry.semantic_difference.as_ref().unwrap();
        assert!(!difference.cli_behavior.trim().is_empty());
        assert!(!difference.http_behavior.trim().is_empty());
    }
}

fn comparison_schema() -> ObservableSchema {
    ObservableSchema {
        operation_family: "fixture".into(),
        version: 1,
        required_fields: vec!["status".into(), "id".into(), "created_at".into()],
        allowed_normalizations: vec![
            NormalizationRule::Envelope,
            NormalizationRule::GeneratedId,
            NormalizationRule::MapKeyOrder,
            NormalizationRule::NondeterministicTimestamp,
        ],
        ordering: ObservableRule::Strict,
        pagination: ObservableRule::Strict,
        time: ObservableRule::Monotonic,
        auth: ObservableRule::Strict,
        errors: ObservableRule::Strict,
        redaction: ObservableRule::Strict,
        nondeterministic_fields: vec!["created_at".into()],
        state: ObservableRule::Strict,
        ignored_fields: vec![],
        retry: ObservableRule::Strict,
        success_cases: vec!["fixture.success".into()],
        error_cases: vec!["fixture.error".into()],
        actors: vec![
            ObservableActor::Authorized,
            ObservableActor::Unauthenticated,
            ObservableActor::Forbidden,
        ],
        case_requirements: vec![ObservableCaseRequirement {
            behavior_case: "fixture.success".into(),
            required_fields: vec!["status".into(), "id".into(), "created_at".into()],
            generated_id_fields: vec!["id".into()],
            invariant_fields: vec![],
        }],
    }
}

#[test]
fn validate_current_and_route_normalization_cover_live_contract() {
    let manifest = validate_current().unwrap();
    assert_eq!(manifest.entries.len(), 72);
    assert_eq!(
        http_ids(&[
            (" get ", "v1/tree/*path/"),
            ("POST", "/v1/health///"),
            ("", "/"),
        ]),
        vec![
            "GET /v1/tree/:path".to_string(),
            "POST /v1/health".to_string(),
            " /".to_string(),
        ]
    );
}
