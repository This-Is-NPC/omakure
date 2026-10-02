use super::*;
use crate::cli_http_parity;
#[test]
fn checked_catalog_is_exhaustive_and_rendering_is_deterministic() {
    let catalog = checked_catalog().unwrap();
    let parity = cli_http_parity::checked_manifest().unwrap();
    catalog.validate(&parity).unwrap();
    assert_eq!(catalog.operations.len(), 74);
    assert_eq!(render_markdown(&catalog), render_markdown(&catalog));
    assert_eq!(
        catalog
            .operations
            .iter()
            .map(|operation| operation.cli.len())
            .sum::<usize>(),
        67
    );
    assert_eq!(
        catalog
            .operations
            .iter()
            .map(|operation| operation.http.len())
            .sum::<usize>(),
        53
    );
    assert_eq!(OPERATION_ID_BASELINE.len(), 74);
}
#[test]
fn trace_and_battery_platform_metadata_match_runtime_guards() {
    let catalog = checked_catalog().unwrap();
    let operation = |entry_id: &str| {
        catalog
            .operations
            .iter()
            .find(|operation| operation.entry_id == entry_id)
            .unwrap()
    };

    let trace = operation("cli-trace");
    assert_eq!(trace.plane, Plane::LocalLifecycle);
    assert_eq!(trace.remote_eligibility, RemoteEligibility::LocalOnly);
    assert_eq!(trace.effect, Effect::Execute);
    assert_eq!(trace.mutability, Mutability::NonIdempotent);

    for entry_id in ["battery-add", "battery-sync"] {
        let battery = operation(entry_id);
        assert!(
            battery.platforms.windows.supported,
            "{entry_id} must support Windows"
        );
        assert_eq!(
            battery.platforms.windows.reason,
            "Supported by the headless Rust runtime and adapter contract."
        );
    }

    let install = operation("battery-install");
    assert!(!install.platforms.windows.supported);
    assert_eq!(
        install.platforms.windows.reason,
        "Battery repository installation and cache operations are Unix-only."
    );
}

#[test]
fn seeded_negative_fixtures_fail_closed() {
    let parity = cli_http_parity::checked_manifest().unwrap();
    let fixtures = [
        (
            include_str!("../../fixtures/operation-catalog/missing-entry.toml"),
            "missing",
        ),
        (
            include_str!("../../fixtures/operation-catalog/orphan-entry.toml"),
            "orphan",
        ),
        (
            include_str!("../../fixtures/operation-catalog/duplicate-operation-id.toml"),
            "duplicate",
        ),
        (
            include_str!("../../fixtures/operation-catalog/invalid-binding.toml"),
            "binding",
        ),
        (
            include_str!("../../fixtures/operation-catalog/invalid-platform.toml"),
            "invalid linux",
        ),
        (
            include_str!("../../fixtures/operation-catalog/invalid-eligibility.toml"),
            "combination",
        ),
        (
            include_str!("../../fixtures/operation-catalog/stable-id-rename.toml"),
            "stable operation_id",
        ),
        (
            include_str!("../../fixtures/operation-catalog/invalid-execute-mutability.toml"),
            "execute effects",
        ),
        (
            include_str!("../../fixtures/operation-catalog/invalid-lifecycle-mutability.toml"),
            "lifecycle effects",
        ),
        (
            include_str!("../../fixtures/operation-catalog/invalid-catalog-version.toml"),
            "unsupported catalog version",
        ),
        (
            include_str!("../../fixtures/operation-catalog/invalid-cli-only-eligibility.toml"),
            "CLI-only parity entries",
        ),
    ];
    for (fixture, expected) in fixtures {
        let error = Catalog::parse_toml(fixture)
            .unwrap()
            .validate(&parity)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{expected}: {error}");
    }
}

#[test]
fn duplicate_operation_ids_are_rejected() {
    let mut catalog = checked_catalog().unwrap();
    catalog.operations[1].operation_id = catalog.operations[0].operation_id.clone();
    let parity = cli_http_parity::checked_manifest().unwrap();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::DuplicateOperationId(_))
    ));
}

#[test]
fn invalid_platform_reason_is_rejected() {
    let mut catalog = checked_catalog().unwrap();
    catalog.operations[0].platforms.linux.reason.clear();
    let parity = cli_http_parity::checked_manifest().unwrap();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::InvalidPlatform { .. })
    ));
}
#[test]
fn current_catalog_and_support_matrix_are_fresh() {
    let catalog = checked_catalog().unwrap();
    let matrix_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SUPPORT_MATRIX_PATH);
    let matrix = std::fs::read_to_string(&matrix_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", matrix_path.display()));
    check_support_matrix_freshness(&catalog, &matrix).unwrap();
    assert!(check_support_matrix_freshness(&catalog, "stale").is_err());
    assert!(matrix.contains("Total operations: 74."));
}
#[test]
fn generated_freshness_accepts_crlf_without_masking_drift() {
    let catalog = checked_catalog().unwrap();
    let generated = render_support_matrix(&catalog);
    let crlf = generated.replace('\n', "\r\n");

    check_support_matrix_freshness(&catalog, &crlf).unwrap();
    assert!(check_support_matrix_freshness(&catalog, &format!("{crlf}drift")).is_err());
}

#[test]
fn catalog_header_and_identity_invariants_fail_closed() {
    let parity = cli_http_parity::checked_manifest().unwrap();
    let mut catalog = checked_catalog().unwrap();
    catalog.schema_version = 2;
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::UnsupportedSchema(2))
    ));

    let mut catalog = checked_catalog().unwrap();
    catalog.catalog_version.clear();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::EmptyField {
            field: "catalog_version",
            ..
        })
    ));

    let mut catalog = checked_catalog().unwrap();
    catalog.catalog_version = "2.0.0".into();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::UnsupportedCatalogVersion { .. })
    ));

    let mut catalog = checked_catalog().unwrap();
    catalog.operations.clear();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::EmptyCatalog)
    ));

    let mut catalog = checked_catalog().unwrap();
    catalog.operations[0].operation_id.clear();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::EmptyField {
            field: "operation_id",
            ..
        })
    ));

    let mut catalog = checked_catalog().unwrap();
    catalog.operations[0].operation_id = "doctor".into();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::InvalidOperationId(_))
    ));

    let mut catalog = checked_catalog().unwrap();
    catalog.operations[0].entry_id.clear();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::EmptyField {
            field: "entry_id",
            ..
        })
    ));

    let mut catalog = checked_catalog().unwrap();
    catalog.operations[1].entry_id = catalog.operations[0].entry_id.clone();
    assert!(matches!(
        catalog.validate(&parity),
        Err(CatalogError::DuplicateEntryId(_))
    ));
}

#[test]
fn catalog_rejects_invalid_binding_and_effect_combinations() {
    let parity = cli_http_parity::checked_manifest().unwrap();
    let validate = |entry_id: &str, mutate: &dyn Fn(&mut Operation)| {
        let mut catalog = checked_catalog().unwrap();
        let operation = catalog
            .operations
            .iter_mut()
            .find(|operation| operation.entry_id == entry_id)
            .unwrap();
        mutate(operation);
        catalog.validate(&parity).unwrap_err()
    };

    assert!(matches!(
        validate("doctor", &|operation| {
            operation.remote_eligibility = RemoteEligibility::LocalOnly
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("doctor", &|operation| {
            operation.mutability = Mutability::Idempotent
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("doctor", &|operation| {
            operation.remote_eligibility = RemoteEligibility::ControlExecute
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("http-health", &|operation| {
            operation.effect = Effect::Mutate
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("node-init", &|operation| {
            operation.effect = Effect::Execute;
            operation.remote_eligibility = RemoteEligibility::ControlObserve;
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("node-init", &|operation| {
            operation.mutability = Mutability::Immutable
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("node-init", &|operation| {
            operation.effect = Effect::Lifecycle;
            operation.remote_eligibility = RemoteEligibility::ControlObserve;
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("cli-trace", &|operation| {
            operation.remote_eligibility = RemoteEligibility::ControlObserve
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("cli-trace", &|operation| {
            operation.mutability = Mutability::Immutable
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("cli-trace", &|operation| {
            operation.plane = Plane::Domain
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("env-create", &|operation| {
            operation.mutability = Mutability::Immutable
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("env-create", &|operation| {
            operation.mutability = Mutability::Idempotent
        }),
        CatalogError::InvalidCombination { .. }
    ));
    assert!(matches!(
        validate("env-replace", &|operation| {
            operation.mutability = Mutability::NonIdempotent
        }),
        CatalogError::InvalidCombination { .. }
    ));
}
