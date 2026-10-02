use super::*;
use crate::cli_http_parity::{self, Manifest as ParityManifest, ParityClass};
use std::collections::{BTreeMap, BTreeSet};

pub fn checked_catalog() -> Result<Catalog, CatalogError> {
    Catalog::parse_toml(include_str!("../../fixtures/operation-catalog.toml"))
}

/// Validate catalog bindings against the current parity manifest and supplied CLI IDs.
pub fn validate_current(cli_ids: &[String]) -> Result<Catalog, CatalogError> {
    let parity = cli_http_parity::validate_current(cli_ids)
        .map_err(|error| CatalogError::Parse(error.to_string()))?;
    let catalog = checked_catalog()?;
    catalog.validate(&parity)?;
    Ok(catalog)
}

impl Catalog {
    pub fn parse_toml(input: &str) -> Result<Self, CatalogError> {
        toml::from_str(input).map_err(|error| CatalogError::Parse(error.to_string()))
    }

    pub fn to_toml(&self) -> Result<String, CatalogError> {
        toml::to_string_pretty(self).map_err(|error| CatalogError::Parse(error.to_string()))
    }

    pub fn validate(&self, parity: &ParityManifest) -> Result<(), CatalogError> {
        validate_catalog_header(self)?;
        let parity_entries = parity
            .entries
            .iter()
            .map(|entry| (entry.entry_id.as_str(), entry))
            .collect();
        let stable_ids = OPERATION_ID_BASELINE.iter().copied().collect();
        let mut state = ValidationState::default();

        for operation in &self.operations {
            validate_operation(operation, &parity_entries, &stable_ids, &mut state)?;
        }
        validate_catalog_completeness(parity, &state)
    }
}
#[derive(Default)]
struct ValidationState<'a> {
    operation_ids: BTreeSet<&'a str>,
    entry_ids: BTreeSet<&'a str>,
    cli_seen: BTreeSet<String>,
    http_seen: BTreeSet<String>,
}

fn validate_catalog_header(catalog: &Catalog) -> Result<(), CatalogError> {
    if catalog.schema_version != SCHEMA_VERSION {
        return Err(CatalogError::UnsupportedSchema(catalog.schema_version));
    }
    if catalog.catalog_version.trim().is_empty() {
        return Err(CatalogError::EmptyField {
            operation: "catalog".into(),
            field: "catalog_version",
        });
    }
    if catalog.catalog_version != CATALOG_VERSION {
        return Err(CatalogError::UnsupportedCatalogVersion {
            expected: CATALOG_VERSION,
            actual: catalog.catalog_version.clone(),
        });
    }
    if catalog.operations.is_empty() {
        return Err(CatalogError::EmptyCatalog);
    }
    Ok(())
}

fn validate_operation<'a>(
    operation: &'a Operation,
    parity_entries: &BTreeMap<&str, &cli_http_parity::ParityEntry>,
    stable_ids: &BTreeMap<&str, &str>,
    state: &mut ValidationState<'a>,
) -> Result<(), CatalogError> {
    validate_operation_identity(operation, state)?;
    let expected = parity_entries
        .get(operation.entry_id.as_str())
        .ok_or_else(|| CatalogError::OrphanEntry(operation.entry_id.clone()))?;
    validate_stable_id(operation, expected, stable_ids)?;
    validate_bindings(
        operation,
        expected.cli_ids.as_slice(),
        expected.http_ids.as_slice(),
        &mut state.cli_seen,
        &mut state.http_seen,
    )?;
    validate_cli_only(operation, expected.class)?;
    validate_platforms(operation)?;
    validate_combination(operation)
}

fn validate_operation_identity<'a>(
    operation: &'a Operation,
    state: &mut ValidationState<'a>,
) -> Result<(), CatalogError> {
    if operation.operation_id.trim().is_empty() {
        return Err(CatalogError::EmptyField {
            operation: operation.entry_id.clone(),
            field: "operation_id",
        });
    }
    if !valid_operation_id(&operation.operation_id) {
        return Err(CatalogError::InvalidOperationId(
            operation.operation_id.clone(),
        ));
    }
    if operation.entry_id.trim().is_empty() {
        return Err(CatalogError::EmptyField {
            operation: operation.operation_id.clone(),
            field: "entry_id",
        });
    }
    if !state.operation_ids.insert(operation.operation_id.as_str()) {
        return Err(CatalogError::DuplicateOperationId(
            operation.operation_id.clone(),
        ));
    }
    if !state.entry_ids.insert(operation.entry_id.as_str()) {
        return Err(CatalogError::DuplicateEntryId(operation.entry_id.clone()));
    }
    Ok(())
}

fn validate_stable_id(
    operation: &Operation,
    expected: &cli_http_parity::ParityEntry,
    stable_ids: &BTreeMap<&str, &str>,
) -> Result<(), CatalogError> {
    let expected_operation_id = stable_ids
        .get(expected.entry_id.as_str())
        .ok_or_else(|| CatalogError::MissingStableId(expected.entry_id.clone()))?;
    if operation.operation_id != *expected_operation_id {
        return Err(CatalogError::StableIdMismatch {
            entry_id: operation.entry_id.clone(),
            expected: (*expected_operation_id).into(),
            actual: operation.operation_id.clone(),
        });
    }
    Ok(())
}

fn validate_cli_only(operation: &Operation, class: ParityClass) -> Result<(), CatalogError> {
    if class == ParityClass::CliOnly
        && !matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
    {
        return Err(CatalogError::InvalidCombination {
            operation: operation.operation_id.clone(),
            reason: "CLI-only parity entries must be local-only".into(),
        });
    }
    Ok(())
}

fn validate_catalog_completeness(
    parity: &ParityManifest,
    state: &ValidationState<'_>,
) -> Result<(), CatalogError> {
    for entry in &parity.entries {
        if !state.entry_ids.contains(entry.entry_id.as_str()) {
            return Err(CatalogError::MissingEntry(entry.entry_id.clone()));
        }
    }
    let parity_cli = parity
        .entries
        .iter()
        .flat_map(|entry| entry.cli_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let parity_http = parity
        .entries
        .iter()
        .flat_map(|entry| entry.http_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    if let Some(id) = state.cli_seen.difference(&parity_cli).next() {
        return Err(CatalogError::UnknownBinding {
            adapter: "cli",
            id: id.clone(),
        });
    }
    if let Some(id) = state.http_seen.difference(&parity_http).next() {
        return Err(CatalogError::UnknownBinding {
            adapter: "http",
            id: id.clone(),
        });
    }
    if let Some(id) = parity_cli.difference(&state.cli_seen).next() {
        return Err(CatalogError::MissingBinding {
            adapter: "cli",
            id: id.clone(),
        });
    }
    if let Some(id) = parity_http.difference(&state.http_seen).next() {
        return Err(CatalogError::MissingBinding {
            adapter: "http",
            id: id.clone(),
        });
    }
    Ok(())
}
fn valid_operation_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("op.") else {
        return false;
    };
    !suffix.is_empty()
        && suffix.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '.')
        })
}

fn validate_bindings(
    operation: &Operation,
    expected_cli: &[String],
    expected_http: &[String],
    cli_seen: &mut BTreeSet<String>,
    http_seen: &mut BTreeSet<String>,
) -> Result<(), CatalogError> {
    for (adapter, actual, expected, seen) in [
        ("cli", &operation.cli, expected_cli, cli_seen),
        ("http", &operation.http, expected_http, http_seen),
    ] {
        let mut local = BTreeSet::new();
        for id in actual {
            if !local.insert(id.as_str()) || !seen.insert(id.clone()) {
                return Err(CatalogError::DuplicateBinding {
                    adapter,
                    id: id.clone(),
                });
            }
            if !expected.iter().any(|expected_id| expected_id == id) {
                return Err(CatalogError::UnknownBinding {
                    adapter,
                    id: id.clone(),
                });
            }
        }
        for id in expected {
            if !actual.iter().any(|actual_id| actual_id == id) {
                return Err(CatalogError::MissingBinding {
                    adapter,
                    id: id.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_platforms(operation: &Operation) -> Result<(), CatalogError> {
    for (name, platform) in [
        ("linux", &operation.platforms.linux),
        ("macos", &operation.platforms.macos),
        ("windows", &operation.platforms.windows),
    ] {
        if platform.reason.trim().is_empty() {
            return Err(CatalogError::InvalidPlatform {
                operation: operation.operation_id.clone(),
                platform: name,
                reason: "reason must be explicit".into(),
            });
        }
    }
    Ok(())
}

fn validate_combination(operation: &Operation) -> Result<(), CatalogError> {
    validate_local_only_http(operation)?;
    validate_plane_combination(operation)?;
    validate_future_contract(operation)?;
    validate_effect_combination(operation)
}

fn invalid_combination(operation: &Operation, reason: &str) -> CatalogError {
    CatalogError::InvalidCombination {
        operation: operation.operation_id.clone(),
        reason: reason.into(),
    }
}

fn validate_local_only_http(operation: &Operation) -> Result<(), CatalogError> {
    if matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
        && !operation.http.is_empty()
    {
        return Err(invalid_combination(
            operation,
            "local-only operations cannot have HTTP bindings",
        ));
    }
    Ok(())
}

fn validate_plane_combination(operation: &Operation) -> Result<(), CatalogError> {
    match operation.plane {
        Plane::LocalLifecycle
            if !matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly) =>
        {
            Err(invalid_combination(
                operation,
                "local-lifecycle operations are local-only",
            ))
        }
        Plane::ServiceObservation
            if !matches!(operation.effect, Effect::Observe)
                || !matches!(operation.mutability, Mutability::Immutable)
                || !matches!(
                    operation.remote_eligibility,
                    RemoteEligibility::ControlObserve
                ) =>
        {
            Err(invalid_combination(
                operation,
                "service-observation must be immutable observe/control-observe",
            ))
        }
        _ => Ok(()),
    }
}

fn validate_future_contract(operation: &Operation) -> Result<(), CatalogError> {
    if matches!(
        operation.remote_eligibility,
        RemoteEligibility::FutureContract
    ) {
        return Err(invalid_combination(
            operation,
            "future-contract is not a claim about a current parity operation",
        ));
    }
    Ok(())
}

fn validate_effect_combination(operation: &Operation) -> Result<(), CatalogError> {
    match operation.effect {
        Effect::Read | Effect::Observe => validate_read_observe(operation),
        Effect::Lifecycle => validate_lifecycle(operation),
        Effect::Execute => validate_execute(operation),
        Effect::Mutate => validate_mutate(operation),
    }
}

fn validate_read_observe(operation: &Operation) -> Result<(), CatalogError> {
    if !matches!(operation.mutability, Mutability::Immutable) {
        return Err(invalid_combination(
            operation,
            "read/observe effects must be immutable",
        ));
    }
    if !matches!(
        operation.remote_eligibility,
        RemoteEligibility::ControlObserve | RemoteEligibility::LocalOnly
    ) {
        return Err(invalid_combination(
            operation,
            "read/observe effects require control-observe or local-only",
        ));
    }
    Ok(())
}

fn validate_lifecycle(operation: &Operation) -> Result<(), CatalogError> {
    if !matches!(operation.mutability, Mutability::NonIdempotent) {
        return Err(invalid_combination(
            operation,
            "lifecycle effects must be non-idempotent",
        ));
    }
    if !matches!(
        operation.remote_eligibility,
        RemoteEligibility::LocalOnly | RemoteEligibility::ControlExecute
    ) {
        return Err(invalid_combination(
            operation,
            "lifecycle effects require local-only or control-execute",
        ));
    }
    if matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
        && !matches!(operation.plane, Plane::LocalLifecycle)
    {
        return Err(invalid_combination(
            operation,
            "local-only lifecycle effects require local-lifecycle",
        ));
    }
    Ok(())
}

fn validate_execute(operation: &Operation) -> Result<(), CatalogError> {
    if !matches!(operation.mutability, Mutability::NonIdempotent) {
        return Err(invalid_combination(
            operation,
            "execute effects must be non-idempotent",
        ));
    }
    if !matches!(
        operation.remote_eligibility,
        RemoteEligibility::ControlExecute | RemoteEligibility::LocalOnly
    ) {
        return Err(invalid_combination(
            operation,
            "execute effects require control-execute or local-only",
        ));
    }
    if matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
        && !matches!(operation.plane, Plane::LocalLifecycle)
    {
        return Err(invalid_combination(
            operation,
            "local-only execute effects require local-lifecycle",
        ));
    }
    Ok(())
}

fn validate_mutate(operation: &Operation) -> Result<(), CatalogError> {
    if matches!(operation.mutability, Mutability::Immutable) {
        return Err(invalid_combination(
            operation,
            "mutate effects cannot be immutable",
        ));
    }
    let expected = match operation.mutability {
        Mutability::Idempotent => RemoteEligibility::ControlConverge,
        Mutability::NonIdempotent => RemoteEligibility::ControlExecute,
        Mutability::Immutable => unreachable!("immutable mutation was rejected above"),
    };
    if operation.remote_eligibility != expected {
        return Err(invalid_combination(
            operation,
            "idempotent mutations require control-converge; non-idempotent mutations require control-execute",
        ));
    }
    Ok(())
}
