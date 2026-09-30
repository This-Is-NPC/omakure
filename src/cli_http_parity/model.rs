use super::SCHEMA_VERSION;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<ParityEntry>,
    #[serde(default)]
    pub schemas: Vec<ObservableSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParityEntry {
    pub entry_id: String,
    pub class: ParityClass,
    pub operation_family: String,
    #[serde(default)]
    pub behavior_case: Option<String>,
    pub docs_anchor: String,
    #[serde(default)]
    pub cli_ids: Vec<String>,
    #[serde(default)]
    pub http_ids: Vec<String>,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub semantic_difference: Option<SemanticDifference>,
    #[serde(default)]
    pub adapter_only: Option<AdapterOnlyRationale>,
}

/// Schema and normalization rules for the observable contract of an operation
/// family.  The schema intentionally describes semantics, rather than Rust
/// response types: both adapters are allowed to choose different envelopes
/// while required values and security decisions remain comparable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservableSchema {
    #[serde(default)]
    pub operation_family: String,
    #[serde(default = "observable_schema_version")]
    pub version: u32,
    #[serde(default)]
    pub required_fields: Vec<String>,
    #[serde(default)]
    pub ignored_fields: Vec<String>,
    #[serde(default)]
    pub nondeterministic_fields: Vec<String>,
    #[serde(default)]
    pub allowed_normalizations: Vec<NormalizationRule>,
    #[serde(default = "default_observable_rule")]
    pub ordering: ObservableRule,
    #[serde(default = "default_observable_rule")]
    pub pagination: ObservableRule,
    #[serde(default = "default_observable_rule")]
    pub time: ObservableRule,
    #[serde(default = "default_observable_rule")]
    pub auth: ObservableRule,
    #[serde(default = "default_observable_rule")]
    pub errors: ObservableRule,
    #[serde(default = "default_observable_rule")]
    pub redaction: ObservableRule,
    #[serde(default = "default_observable_rule")]
    pub state: ObservableRule,
    #[serde(default = "default_observable_rule")]
    pub retry: ObservableRule,
    #[serde(default)]
    pub success_cases: Vec<String>,
    #[serde(default)]
    pub error_cases: Vec<String>,
    #[serde(default)]
    pub actors: Vec<ObservableActor>,
    #[serde(default)]
    pub case_requirements: Vec<ObservableCaseRequirement>,
}

/// Semantic fields and invariants required for one executable behavior case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ObservableCaseRequirement {
    pub behavior_case: String,
    #[serde(default)]
    pub required_fields: Vec<String>,

    #[serde(default)]
    pub generated_id_fields: Vec<String>,
    #[serde(default)]
    pub invariant_fields: Vec<String>,
}

fn observable_schema_version() -> u32 {
    SCHEMA_VERSION
}

fn default_observable_rule() -> ObservableRule {
    ObservableRule::Strict
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ObservableRule {
    #[default]
    Strict,
    Presence,
    Monotonic,
    Bounded,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NormalizationRule {
    Envelope,
    GeneratedId,
    MapKeyOrder,
    NondeterministicTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservableActor {
    Authorized,
    Unauthenticated,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParityClass {
    Exact,
    SemanticMismatch,
    CliOnly,
    HttpOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticDifference {
    pub kind: String,
    pub cli_behavior: String,
    pub http_behavior: String,
    #[serde(default)]
    pub impact: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterOnlyRationale {
    pub auth: String,
    pub lifecycle: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceInventory<'a> {
    pub cli_ids: &'a [String],
    pub http_ids: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    Parse(String),
    UnsupportedSchema(u32),
    EmptyManifest,
    EmptyField { entry: String, field: &'static str },
    DuplicateEntry(String),
    DuplicateAnchor(String),
    DuplicateSurface { surface: String },
    UnknownSurface { side: &'static str, surface: String },
    MissingSurface { side: &'static str, surface: String },
    WrongClassSides { entry: String, class: ParityClass },
    MissingBehaviorCase(String),
    MissingSemanticDifference(String),
    MissingAdapterRationale(String),
    IncompatibleSemanticDifference(String),
    IncompatibleAdapterOnly(String),
    WildcardSurface { entry: String, surface: String },
    DocsAnchorMissing { entry: String, anchor: String },
    MissingObservableSchema { family: String },
    DuplicateObservableSchema(String),
    UnknownObservableSchema(String),
    InvalidObservableSchema { family: String, reason: String },
    DuplicateBehaviorCase(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ManifestError {}

#[derive(Debug, Clone, Copy)]
pub(super) struct InventorySets<'a> {
    pub(super) cli: &'a BTreeSet<&'a str>,
    pub(super) http: &'a BTreeSet<&'a str>,
}
