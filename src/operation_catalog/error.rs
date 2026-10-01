use std::fmt;

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
            Self::Parse(_)
            | Self::UnsupportedSchema(_)
            | Self::UnsupportedCatalogVersion { .. }
            | Self::EmptyCatalog => fmt_catalog_error(self, f),
            Self::EmptyField { .. }
            | Self::DuplicateOperationId(_)
            | Self::InvalidOperationId(_)
            | Self::DuplicateEntryId(_)
            | Self::MissingEntry(_)
            | Self::OrphanEntry(_)
            | Self::StableIdMismatch { .. }
            | Self::MissingStableId(_) => fmt_identity_error(self, f),
            Self::DuplicateBinding { .. }
            | Self::MissingBinding { .. }
            | Self::UnknownBinding { .. } => fmt_binding_error(self, f),
            Self::InvalidCombination { .. } | Self::InvalidPlatform { .. } => {
                fmt_constraint_error(self, f)
            }
        }
    }
}

fn fmt_catalog_error(error: &CatalogError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match error {
        CatalogError::Parse(reason) => write!(f, "catalog parse error: {reason}"),
        CatalogError::UnsupportedSchema(version) => {
            write!(f, "unsupported catalog schema version {version}")
        }
        CatalogError::UnsupportedCatalogVersion { expected, actual } => {
            write!(
                f,
                "unsupported catalog version {actual}; expected {expected}"
            )
        }
        CatalogError::EmptyCatalog => f.write_str("catalog has no operations"),
        _ => unreachable!("non-catalog error passed to fmt_catalog_error"),
    }
}

fn fmt_identity_error(error: &CatalogError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match error {
        CatalogError::EmptyField { operation, field } => {
            write!(f, "{operation} has empty {field}")
        }
        CatalogError::DuplicateOperationId(id) => write!(f, "duplicate operation_id {id}"),
        CatalogError::InvalidOperationId(id) => write!(f, "invalid operation_id {id}"),
        CatalogError::DuplicateEntryId(id) => write!(f, "duplicate entry_id {id}"),
        CatalogError::MissingEntry(id) => write!(f, "catalog is missing parity entry {id}"),
        CatalogError::OrphanEntry(id) => write!(f, "catalog contains orphan entry {id}"),
        CatalogError::StableIdMismatch {
            entry_id,
            expected,
            actual,
        } => write!(
            f,
            "entry {entry_id} must retain stable operation_id {expected}, found {actual}"
        ),
        CatalogError::MissingStableId(id) => {
            write!(f, "catalog entry {id} has no stable operation_id baseline")
        }
        _ => unreachable!("non-identity error passed to fmt_identity_error"),
    }
}

fn fmt_binding_error(error: &CatalogError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match error {
        CatalogError::DuplicateBinding { adapter, id } => {
            write!(f, "duplicate {adapter} binding {id}")
        }
        CatalogError::MissingBinding { adapter, id } => {
            write!(f, "missing {adapter} binding {id}")
        }
        CatalogError::UnknownBinding { adapter, id } => {
            write!(f, "unknown {adapter} binding {id}")
        }
        _ => unreachable!("non-binding error passed to fmt_binding_error"),
    }
}

fn fmt_constraint_error(error: &CatalogError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match error {
        CatalogError::InvalidCombination { operation, reason } => write!(
            f,
            "{operation} has invalid plane/eligibility/effect combination: {reason}"
        ),
        CatalogError::InvalidPlatform {
            operation,
            platform,
            reason,
        } => write!(f, "{operation} has invalid {platform} support: {reason}"),
        _ => unreachable!("non-constraint error passed to fmt_constraint_error"),
    }
}

impl std::error::Error for CatalogError {}
