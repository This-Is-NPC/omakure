//! Versioned CLI/HTTP parity contract.
//!
//! The checked-in manifest is deliberately boring: every surface is named
//! explicitly, while this module owns the invariants that make omissions and
//! accidental overlaps impossible.  Markdown is rendered from the manifest;
//! it is never a second source of truth.

mod compare;
mod docs;
mod manifest;
mod model;
mod schema;

pub use compare::{compare_observables_for_case, validate_observables_for_case, ObservableError};
pub use docs::{check_docs_freshness, render_markdown};
pub use manifest::http_ids;
pub use model::{
    AdapterOnlyRationale, Manifest, ManifestError, NormalizationRule, ObservableActor,
    ObservableCaseRequirement, ObservableRule, ObservableSchema, ParityClass, ParityEntry,
    SemanticDifference, SurfaceInventory,
};
pub use schema::{checked_registry, BehaviorCase};

pub const SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_PATH: &str = "fixtures/cli-http-parity.toml";
pub const DOCS_PATH: &str = "docs/cli-http-parity.md";

/// Parse the checked-in contract.
pub fn checked_manifest() -> Result<Manifest, ManifestError> {
    Manifest::parse_toml(include_str!("../../fixtures/cli-http-parity.toml"))
}

pub fn current_cli_ids() -> Vec<String> {
    crate::inventory::command_inventory()
        .into_iter()
        .filter(|command| command.subcommands.is_empty())
        .map(|command| command.id)
        .collect()
}

/// Current HTTP IDs supplied by the shared route inventory.
pub fn current_http_ids() -> Vec<String> {
    http_ids(crate::inventory::HTTP_ROUTE_INVENTORY)
}
/// Validate the checked-in manifest against both live structural inventories.
pub fn validate_current() -> Result<Manifest, ManifestError> {
    let manifest = checked_manifest()?;
    let cli_ids = current_cli_ids();
    let http_ids = current_http_ids();
    manifest.validate(SurfaceInventory {
        cli_ids: &cli_ids,
        http_ids: &http_ids,
    })?;
    Ok(manifest)
}

#[cfg(test)]
mod tests;
