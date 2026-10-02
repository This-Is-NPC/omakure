//! Canonical, versioned operation metadata.
//!
//! The operation catalog is deliberately separate from presentation metadata and
//! parity. Each record points at exactly one parity entry and describes the
//! operation's ownership plane, remote safety, effect, adapters, and static
//! platform support.

mod error;
mod ids;
mod model;
mod render;
mod validate;

pub use error::CatalogError;
pub use ids::{
    CATALOG_VERSION, DOCS_PATH, MANIFEST_PATH, OPERATION_ID_BASELINE, SCHEMA_VERSION,
    SUPPORT_MATRIX_PATH,
};
pub use model::{
    Catalog, Effect, Mutability, Operation, Plane, PlatformSupport, PlatformSupportSet,
    RemoteEligibility,
};
pub use render::{
    check_docs_freshness, check_support_matrix_freshness, render_markdown, render_support_matrix,
};
pub use validate::{checked_catalog, validate_current};

#[cfg(test)]
mod tests;
