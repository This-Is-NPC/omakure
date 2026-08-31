//! Canonical, versioned operation metadata.
//!
//! The operation catalog is deliberately separate from presentation metadata and
//! parity.  Each record points at exactly one parity entry and describes the
//! operation's ownership plane, remote safety, effect, adapters, and static
//! platform support.

use crate::cli_http_parity::{self, Manifest as ParityManifest, ParityClass, SurfaceInventory};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;
pub const CATALOG_VERSION: &str = "1.0.0";
pub const MANIFEST_PATH: &str = "fixtures/operation-catalog.toml";
pub const DOCS_PATH: &str = "docs/operation-catalog.md";
pub const SUPPORT_MATRIX_PATH: &str = "docs/operation-support-matrix.md";

/// Immutable operation identities.  `entry_id` is the parity-facing name and
/// `operation_id` is the consumer-facing stable key; changing either mapping
/// is a compatibility event rather than an ordinary catalog edit.
pub const OPERATION_ID_BASELINE: &[(&str, &str)] = &[
    ("doctor", "op.doctor"),
    ("scripts", "op.scripts"),
    ("describe", "op.describe"),
    ("history-list", "op.history-list"),
    ("history-show", "op.history-show"),
    ("history-traces", "op.history-traces"),
    ("history-stats", "op.history-stats"),
    ("queue-add", "op.queue-add"),
    ("queue-cancel", "op.queue-cancel"),
    ("queue-dead-letter", "op.queue-dead-letter"),
    ("env-list", "op.env-list"),
    ("env-create", "op.env-create"),
    ("env-show", "op.env-show"),
    ("env-replace", "op.env-replace"),
    ("env-set", "op.env-set"),
    ("env-remove", "op.env-remove"),
    ("env-activate", "op.env-activate"),
    ("env-deactivate", "op.env-deactivate"),
    ("env-delete", "op.env-delete"),
    ("battery-list", "op.battery-list"),
    ("battery-inspect", "op.battery-inspect"),
    ("battery-scripts", "op.battery-scripts"),
    ("battery-remove", "op.battery-remove"),
    ("node-init", "op.node-init"),
    ("node-status", "op.node-status"),
    ("node-peers", "op.node-peers"),
    ("node-trust", "op.node-trust"),
    ("node-capabilities", "op.node-capabilities"),
    ("node-revoke", "op.node-revoke"),
    ("node-health", "op.node-health"),
    ("node-signals", "op.node-signals"),
    ("node-baseline-push", "op.node-baseline-push"),
    ("node-baseline-rollback", "op.node-baseline-rollback"),
    ("node-enroll-approve", "op.node-enroll-approve"),
    ("node-enroll-reject", "op.node-enroll-reject"),
    ("config", "op.config"),
    ("search", "op.search"),
    ("battery-add", "op.battery-add"),
    ("battery-sync", "op.battery-sync"),
    ("battery-install", "op.battery-install"),
    ("node-discovery", "op.node-discovery"),
    ("node-cue", "op.node-cue"),
    ("node-enroll-request", "op.node-enroll-request"),
    ("http-node-enrollments", "op.http-node-enrollments"),
    ("node-enroll-apply", "op.node-enroll-apply"),
    ("http-health", "op.http-health"),
    ("http-ready", "op.http-ready"),
    ("http-admin-status", "op.http-admin-status"),
    ("http-workspace", "op.http-workspace"),
    ("http-tree-root", "op.http-tree-root"),
    ("http-tree-path", "op.http-tree-path"),
    ("http-secrets", "op.http-secrets"),
    ("cli-api", "op.cli-api"),
    ("cli-completion", "op.cli-completion"),
    ("cli-help-ai", "op.cli-help-ai"),
    ("cli-history-tail", "op.cli-history-tail"),
    ("cli-init", "op.cli-init"),
    ("cli-node-authority-create", "op.cli-node-authority-create"),
    ("cli-node-authority-issue", "op.cli-node-authority-issue"),
    ("cli-node-authority-show", "op.cli-node-authority-show"),
    (
        "cli-node-baseline-create-key",
        "op.cli-node-baseline-create-key",
    ),
    ("cli-node-baseline-publish", "op.cli-node-baseline-publish"),
    ("cli-node-direct-probe", "op.cli-node-direct-probe"),
    ("cli-node-reset", "op.cli-node-reset"),
    ("cli-node-serve", "op.cli-node-serve"),
    ("cli-queue-worker", "op.cli-queue-worker"),
    ("cli-run", "op.cli-run"),
    ("cli-serve", "op.cli-serve"),
    ("cli-token-generate", "op.cli-token-generate"),
    ("cli-trace", "op.cli-trace"),
    ("cli-uninstall", "op.cli-uninstall"),
    ("cli-update", "op.cli-update"),
];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub catalog_version: String,
    #[serde(default)]
    pub operations: Vec<Operation>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub operation_id: String,
    pub entry_id: String,
    pub plane: Plane,
    pub remote_eligibility: RemoteEligibility,
    pub effect: Effect,
    pub mutability: Mutability,
    #[serde(default)]
    pub cli: Vec<String>,
    #[serde(default)]
    pub http: Vec<String>,
    pub platforms: PlatformSupportSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Plane {
    Domain,
    LocalLifecycle,
    ServiceObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteEligibility {
    LocalOnly,
    ControlObserve,
    ControlExecute,
    ControlConverge,
    FutureContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Read,
    Observe,
    Mutate,
    Execute,
    Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mutability {
    Immutable,
    Idempotent,
    NonIdempotent,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSupportSet {
    pub linux: PlatformSupport,
    pub macos: PlatformSupport,
    pub windows: PlatformSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSupport {
    pub supported: bool,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    Parse(String),
    UnsupportedSchema(u32),
    UnsupportedCatalogVersion {
        expected: &'static str,
        actual: String,
    },
    EmptyCatalog,
    EmptyField {
        operation: String,
        field: &'static str,
    },
    DuplicateOperationId(String),
    InvalidOperationId(String),
    DuplicateEntryId(String),
    MissingEntry(String),
    OrphanEntry(String),
    StableIdMismatch {
        entry_id: String,
        expected: String,
        actual: String,
    },
    MissingStableId(String),
    DuplicateBinding {
        adapter: &'static str,
        id: String,
    },
    MissingBinding {
        adapter: &'static str,
        id: String,
    },
    UnknownBinding {
        adapter: &'static str,
        id: String,
    },
    InvalidCombination {
        operation: String,
        reason: String,
    },
    InvalidPlatform {
        operation: String,
        platform: &'static str,
        reason: String,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(reason) => write!(f, "catalog parse error: {reason}"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported catalog schema version {version}")
            }
            Self::UnsupportedCatalogVersion { expected, actual } => write!(
                f,
                "unsupported catalog version {actual}; expected {expected}"
            ),
            Self::EmptyCatalog => f.write_str("catalog has no operations"),
            Self::EmptyField { operation, field } => write!(f, "{operation} has empty {field}"),
            Self::DuplicateOperationId(id) => write!(f, "duplicate operation_id {id}"),
            Self::InvalidOperationId(id) => write!(f, "invalid operation_id {id}"),
            Self::DuplicateEntryId(id) => write!(f, "duplicate entry_id {id}"),
            Self::MissingEntry(id) => write!(f, "catalog is missing parity entry {id}"),
            Self::OrphanEntry(id) => write!(f, "catalog contains orphan entry {id}"),
            Self::StableIdMismatch {
                entry_id,
                expected,
                actual,
            } => write!(
                f,
                "entry {entry_id} must retain stable operation_id {expected}, found {actual}"
            ),
            Self::MissingStableId(id) => {
                write!(f, "catalog entry {id} has no stable operation_id baseline")
            }
            Self::DuplicateBinding { adapter, id } => write!(f, "duplicate {adapter} binding {id}"),
            Self::MissingBinding { adapter, id } => write!(f, "missing {adapter} binding {id}"),
            Self::UnknownBinding { adapter, id } => write!(f, "unknown {adapter} binding {id}"),
            Self::InvalidCombination { operation, reason } => write!(
                f,
                "{operation} has invalid plane/eligibility/effect combination: {reason}"
            ),
            Self::InvalidPlatform {
                operation,
                platform,
                reason,
            } => write!(f, "{operation} has invalid {platform} support: {reason}"),
        }
    }
}

impl std::error::Error for CatalogError {}

pub fn checked_catalog() -> Result<Catalog, CatalogError> {
    Catalog::parse_toml(include_str!("../fixtures/operation-catalog.toml"))
}

pub fn validate_current() -> Result<Catalog, CatalogError> {
    let parity = cli_http_parity::checked_manifest()
        .map_err(|error| CatalogError::Parse(error.to_string()))?;
    parity
        .validate(SurfaceInventory {
            cli_ids: &cli_http_parity::current_cli_ids(),
            http_ids: &cli_http_parity::current_http_ids(),
        })
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
        if self.schema_version != SCHEMA_VERSION {
            return Err(CatalogError::UnsupportedSchema(self.schema_version));
        }
        if self.catalog_version.trim().is_empty() {
            return Err(CatalogError::EmptyField {
                operation: "catalog".into(),
                field: "catalog_version",
            });
        }
        if self.catalog_version != CATALOG_VERSION {
            return Err(CatalogError::UnsupportedCatalogVersion {
                expected: CATALOG_VERSION,
                actual: self.catalog_version.clone(),
            });
        }
        if self.operations.is_empty() {
            return Err(CatalogError::EmptyCatalog);
        }
        let parity_entries: BTreeMap<&str, &cli_http_parity::ParityEntry> = parity
            .entries
            .iter()
            .map(|entry| (entry.entry_id.as_str(), entry))
            .collect();
        let stable_ids: BTreeMap<&str, &str> = OPERATION_ID_BASELINE.iter().copied().collect();
        let mut operations = BTreeSet::new();
        let mut entries = BTreeSet::new();
        let mut cli_seen = BTreeSet::new();
        let mut http_seen = BTreeSet::new();
        for operation in &self.operations {
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
            if !operations.insert(operation.operation_id.as_str()) {
                return Err(CatalogError::DuplicateOperationId(
                    operation.operation_id.clone(),
                ));
            }
            if !entries.insert(operation.entry_id.as_str()) {
                return Err(CatalogError::DuplicateEntryId(operation.entry_id.clone()));
            }
            let expected = parity_entries
                .get(operation.entry_id.as_str())
                .ok_or_else(|| CatalogError::OrphanEntry(operation.entry_id.clone()))?;
            let expected_operation_id = stable_ids
                .get(operation.entry_id.as_str())
                .ok_or_else(|| CatalogError::MissingStableId(operation.entry_id.clone()))?;
            if operation.operation_id != *expected_operation_id {
                return Err(CatalogError::StableIdMismatch {
                    entry_id: operation.entry_id.clone(),
                    expected: (*expected_operation_id).into(),
                    actual: operation.operation_id.clone(),
                });
            }
            validate_bindings(
                operation,
                expected.cli_ids.as_slice(),
                expected.http_ids.as_slice(),
                &mut cli_seen,
                &mut http_seen,
            )?;
            if expected.class == ParityClass::CliOnly
                && !matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
            {
                return Err(CatalogError::InvalidCombination {
                    operation: operation.operation_id.clone(),
                    reason: "CLI-only parity entries must be local-only".into(),
                });
            }
            validate_platforms(operation)?;
            validate_combination(operation)?;
        }
        for entry in &parity.entries {
            if !entries.contains(entry.entry_id.as_str()) {
                return Err(CatalogError::MissingEntry(entry.entry_id.clone()));
            }
        }
        let parity_cli: BTreeSet<String> = parity
            .entries
            .iter()
            .flat_map(|entry| entry.cli_ids.iter().cloned())
            .collect();
        let parity_http: BTreeSet<String> = parity
            .entries
            .iter()
            .flat_map(|entry| entry.http_ids.iter().cloned())
            .collect();
        if let Some(id) = cli_seen.difference(&parity_cli).next() {
            return Err(CatalogError::UnknownBinding {
                adapter: "cli",
                id: id.clone(),
            });
        }
        if let Some(id) = http_seen.difference(&parity_http).next() {
            return Err(CatalogError::UnknownBinding {
                adapter: "http",
                id: id.clone(),
            });
        }
        if let Some(id) = parity_cli.difference(&cli_seen).next() {
            return Err(CatalogError::MissingBinding {
                adapter: "cli",
                id: id.clone(),
            });
        }
        if let Some(id) = parity_http.difference(&http_seen).next() {
            return Err(CatalogError::MissingBinding {
                adapter: "http",
                id: id.clone(),
            });
        }
        Ok(())
    }
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
    let invalid = |reason: &str| CatalogError::InvalidCombination {
        operation: operation.operation_id.clone(),
        reason: reason.into(),
    };
    if matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
        && !operation.http.is_empty()
    {
        return Err(invalid("local-only operations cannot have HTTP bindings"));
    }
    match operation.plane {
        Plane::LocalLifecycle
            if !matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly) =>
        {
            return Err(invalid("local-lifecycle operations are local-only"))
        }
        Plane::ServiceObservation
            if !matches!(operation.effect, Effect::Observe)
                || !matches!(operation.mutability, Mutability::Immutable)
                || !matches!(
                    operation.remote_eligibility,
                    RemoteEligibility::ControlObserve
                ) =>
        {
            return Err(invalid(
                "service-observation must be immutable observe/control-observe",
            ))
        }
        _ => {}
    }
    if matches!(
        operation.remote_eligibility,
        RemoteEligibility::FutureContract
    ) {
        return Err(invalid(
            "future-contract is not a claim about a current parity operation",
        ));
    }
    match operation.effect {
        Effect::Read | Effect::Observe => {
            if !matches!(operation.mutability, Mutability::Immutable) {
                return Err(invalid("read/observe effects must be immutable"));
            }
            if !matches!(
                operation.remote_eligibility,
                RemoteEligibility::ControlObserve | RemoteEligibility::LocalOnly
            ) {
                return Err(invalid(
                    "read/observe effects require control-observe or local-only",
                ));
            }
        }
        Effect::Lifecycle => {
            if !matches!(operation.mutability, Mutability::NonIdempotent) {
                return Err(invalid("lifecycle effects must be non-idempotent"));
            }
            if !matches!(
                operation.remote_eligibility,
                RemoteEligibility::LocalOnly | RemoteEligibility::ControlExecute
            ) {
                return Err(invalid(
                    "lifecycle effects require local-only or control-execute",
                ));
            }
            if matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
                && !matches!(operation.plane, Plane::LocalLifecycle)
            {
                return Err(invalid(
                    "local-only lifecycle effects require local-lifecycle",
                ));
            }
        }
        Effect::Execute => {
            if !matches!(operation.mutability, Mutability::NonIdempotent) {
                return Err(invalid("execute effects must be non-idempotent"));
            }
            if !matches!(
                operation.remote_eligibility,
                RemoteEligibility::ControlExecute | RemoteEligibility::LocalOnly
            ) {
                return Err(invalid(
                    "execute effects require control-execute or local-only",
                ));
            }
            if matches!(operation.remote_eligibility, RemoteEligibility::LocalOnly)
                && !matches!(operation.plane, Plane::LocalLifecycle)
            {
                return Err(invalid(
                    "local-only execute effects require local-lifecycle",
                ));
            }
        }
        Effect::Mutate => {
            if matches!(operation.mutability, Mutability::Immutable) {
                return Err(invalid("mutate effects cannot be immutable"));
            }
            let expected = if matches!(operation.mutability, Mutability::Idempotent) {
                RemoteEligibility::ControlConverge
            } else {
                RemoteEligibility::ControlExecute
            };
            if operation.remote_eligibility != expected {
                return Err(invalid(
                    "idempotent mutations require control-converge; non-idempotent mutations require control-execute",
                ));
            }
        }
    }
    Ok(())
}

pub fn render_markdown(catalog: &Catalog) -> String {
    let mut output = format!(
        "<!-- GENERATED FILE: scripts/tasks/operation-catalog --write -->\n# Operation catalog\n\nCatalog version: `{}`; schema version: `{}`.\n\n",
        catalog.catalog_version, catalog.schema_version
    );
    output.push_str("| Operation ID | Parity entry | Plane | Remote eligibility | Effect | Mutability | CLI | HTTP |\n|---|---|---|---|---|---|---|---|\n");
    let mut operations = catalog.operations.iter().collect::<Vec<_>>();
    operations.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    for operation in operations {
        output.push_str(&format!(
            "| `{}` | `{}` | `{}` | `{}` | `{}` | `{}` | {} | {} |\n",
            operation.operation_id,
            operation.entry_id,
            display(&operation.plane),
            display(&operation.remote_eligibility),
            display(&operation.effect),
            display(&operation.mutability),
            join(&operation.cli),
            join(&operation.http)
        ));
    }
    output.push_str(&format!(
        "\nTotal operations: {}.\n",
        catalog.operations.len()
    ));
    output
}

pub fn render_support_matrix(catalog: &Catalog) -> String {
    let mut output = format!(
        "<!-- GENERATED FILE: scripts/tasks/operation-catalog --write -->\n# Operation support matrix\n\nCatalog version: `{}`; schema version: `{}`.\n\n| Operation ID | Linux | macOS | Windows |\n|---|---|---|---|\n",
        catalog.catalog_version, catalog.schema_version
    );
    let mut operations = catalog.operations.iter().collect::<Vec<_>>();
    operations.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    for operation in operations {
        output.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            operation.operation_id,
            support(&operation.platforms.linux),
            support(&operation.platforms.macos),
            support(&operation.platforms.windows)
        ));
    }
    output.push_str(&format!(
        "\nTotal operations: {}.\n",
        catalog.operations.len()
    ));
    output
}

fn support(platform: &PlatformSupport) -> String {
    format!(
        "{} — {}",
        if platform.supported {
            "supported"
        } else {
            "unsupported"
        },
        platform.reason
    )
}
fn join(values: &[String]) -> String {
    if values.is_empty() {
        "—".into()
    } else {
        values
            .iter()
            .map(|value| format!("`{value}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}
fn display<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .expect("catalog enum serializes")
        .as_str()
        .expect("catalog enum is string")
        .into()
}

pub fn check_docs_freshness(catalog: &Catalog, docs: &str) -> Result<(), CatalogError> {
    if docs != render_markdown(catalog) {
        return Err(CatalogError::Parse(
            "generated operation catalog documentation is stale; regenerate from the catalog"
                .into(),
        ));
    }
    Ok(())
}

pub fn check_support_matrix_freshness(
    catalog: &Catalog,
    support_matrix: &str,
) -> Result<(), CatalogError> {
    if support_matrix != render_support_matrix(catalog) {
        return Err(CatalogError::Parse(
            "generated operation support matrix is stale; regenerate from the catalog".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_catalog_is_exhaustive_and_rendering_is_deterministic() {
        let catalog = checked_catalog().unwrap();
        let parity = cli_http_parity::checked_manifest().unwrap();
        catalog.validate(&parity).unwrap();
        assert_eq!(catalog.operations.len(), 72);
        assert_eq!(render_markdown(&catalog), render_markdown(&catalog));
        assert_eq!(
            catalog
                .operations
                .iter()
                .map(|operation| operation.cli.len())
                .sum::<usize>(),
            65
        );
        assert_eq!(
            catalog
                .operations
                .iter()
                .map(|operation| operation.http.len())
                .sum::<usize>(),
            53
        );
        assert_eq!(OPERATION_ID_BASELINE.len(), 72);
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
                include_str!("../fixtures/operation-catalog/missing-entry.toml"),
                "missing",
            ),
            (
                include_str!("../fixtures/operation-catalog/orphan-entry.toml"),
                "orphan",
            ),
            (
                include_str!("../fixtures/operation-catalog/duplicate-operation-id.toml"),
                "duplicate",
            ),
            (
                include_str!("../fixtures/operation-catalog/invalid-binding.toml"),
                "binding",
            ),
            (
                include_str!("../fixtures/operation-catalog/invalid-platform.toml"),
                "invalid linux",
            ),
            (
                include_str!("../fixtures/operation-catalog/invalid-eligibility.toml"),
                "combination",
            ),
            (
                include_str!("../fixtures/operation-catalog/stable-id-rename.toml"),
                "stable operation_id",
            ),
            (
                include_str!("../fixtures/operation-catalog/invalid-execute-mutability.toml"),
                "execute effects",
            ),
            (
                include_str!("../fixtures/operation-catalog/invalid-lifecycle-mutability.toml"),
                "lifecycle effects",
            ),
            (
                include_str!("../fixtures/operation-catalog/invalid-catalog-version.toml"),
                "unsupported catalog version",
            ),
            (
                include_str!("../fixtures/operation-catalog/invalid-cli-only-eligibility.toml"),
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
}
