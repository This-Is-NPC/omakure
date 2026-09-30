//! Versioned CLI/HTTP parity contract.
//!
//! The checked-in manifest is deliberately boring: every surface is named
//! explicitly, while this module owns the invariants that make omissions and
//! accidental overlaps impossible.  Markdown is rendered from the manifest;
//! it is never a second source of truth.

use crate::cli::inventory::normalize_generated_text;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_PATH: &str = "fixtures/cli-http-parity.toml";
pub const DOCS_PATH: &str = "docs/cli-http-parity.md";

/// Parse the checked-in contract.
pub fn checked_manifest() -> Result<Manifest, ManifestError> {
    Manifest::parse_toml(include_str!("../fixtures/cli-http-parity.toml"))
}

pub fn current_cli_ids() -> Vec<String> {
    crate::cli::inventory::command_inventory()
        .into_iter()
        .filter(|command| command.subcommands.is_empty())
        .map(|command| command.id)
        .collect()
}

/// Current HTTP IDs supplied by the router-owned route inventory.
pub fn current_http_ids() -> Vec<String> {
    http_ids(crate::cli::api::HTTP_ROUTE_INVENTORY)
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
struct InventorySets<'a> {
    cli: &'a BTreeSet<&'a str>,
    http: &'a BTreeSet<&'a str>,
}

impl Manifest {
    pub fn parse_toml(input: &str) -> Result<Self, ManifestError> {
        toml::from_str(input).map_err(|error| ManifestError::Parse(error.to_string()))
    }

    pub fn to_toml(&self) -> Result<String, ManifestError> {
        toml::to_string_pretty(self).map_err(|error| ManifestError::Parse(error.to_string()))
    }

    pub fn validate(&self, inventory: SurfaceInventory<'_>) -> Result<(), ManifestError> {
        self.validate_header()?;
        let cli_inventory: BTreeSet<_> = inventory.cli_ids.iter().map(String::as_str).collect();
        let http_inventory: BTreeSet<_> = inventory.http_ids.iter().map(String::as_str).collect();
        let sets = InventorySets {
            cli: &cli_inventory,
            http: &http_inventory,
        };
        let mut seen_entries = BTreeSet::new();
        let mut seen_anchors = BTreeSet::new();
        let mut seen_cli = BTreeSet::new();
        let mut seen_http = BTreeSet::new();
        for entry in &self.entries {
            self.validate_entry(
                entry,
                sets,
                &mut seen_entries,
                &mut seen_anchors,
                &mut seen_cli,
                &mut seen_http,
            )?;
        }
        validate_complete("cli_ids", &cli_inventory, &seen_cli)?;
        validate_complete("http_ids", &http_inventory, &seen_http)?;
        self.validate_observable_registry()
    }

    fn validate_header(&self) -> Result<(), ManifestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema(self.schema_version));
        }
        if self.entries.is_empty() {
            return Err(ManifestError::EmptyManifest);
        }
        Ok(())
    }

    fn validate_entry(
        &self,
        entry: &ParityEntry,
        sets: InventorySets<'_>,
        seen_entries: &mut BTreeSet<String>,
        seen_anchors: &mut BTreeSet<String>,
        seen_cli: &mut BTreeSet<String>,
        seen_http: &mut BTreeSet<String>,
    ) -> Result<(), ManifestError> {
        validate_entry_identity(entry, seen_entries, seen_anchors)?;
        validate_surfaces(entry, "cli_ids", &entry.cli_ids, sets.cli, seen_cli)?;
        validate_surfaces(entry, "http_ids", &entry.http_ids, sets.http, seen_http)?;
        validate_entry_class(entry)?;
        validate_entry_metadata(entry)
    }

    /// Validate schemas and build the executable case registry from manifest
    /// entries. Every compared behavior case has one and only one semantic
    /// requirement in its family's schema.
    pub fn validate_observable_registry(&self) -> Result<(), ManifestError> {
        let mut schemas = BTreeMap::new();
        for schema in &self.schemas {
            validate_observable_schema(schema)?;
            if schemas
                .insert(schema.operation_family.clone(), schema)
                .is_some()
            {
                return Err(ManifestError::DuplicateObservableSchema(
                    schema.operation_family.clone(),
                ));
            }
        }
        let mut cases = BTreeSet::new();
        let mut compared_families = BTreeSet::new();
        let mut compared_by_family: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for entry in &self.entries {
            if !matches!(
                entry.class,
                ParityClass::Exact | ParityClass::SemanticMismatch
            ) {
                continue;
            }
            let family = entry.operation_family.as_str();
            compared_families.insert(family);
            let schema =
                schemas
                    .get(family)
                    .ok_or_else(|| ManifestError::MissingObservableSchema {
                        family: family.to_string(),
                    })?;
            let case = entry
                .behavior_case
                .as_deref()
                .filter(|case| !case.trim().is_empty())
                .ok_or_else(|| ManifestError::MissingBehaviorCase(entry.entry_id.clone()))?;
            if !cases.insert(case.to_string()) {
                return Err(ManifestError::DuplicateBehaviorCase(case.to_string()));
            }
            compared_by_family.entry(family).or_default().insert(case);
            if !schema
                .case_requirements
                .iter()
                .any(|requirement| requirement.behavior_case == case)
            {
                return Err(ManifestError::InvalidObservableSchema {
                    family: family.to_string(),
                    reason: format!("missing case requirement for {case}"),
                });
            }
        }
        for schema in &self.schemas {
            let family = schema.operation_family.as_str();
            if !compared_families.contains(family) {
                return Err(ManifestError::UnknownObservableSchema(
                    schema.operation_family.clone(),
                ));
            }
            let expected = compared_by_family.get(family).cloned().unwrap_or_default();
            let actual: BTreeSet<_> = schema
                .case_requirements
                .iter()
                .map(|requirement| requirement.behavior_case.as_str())
                .collect();
            if actual != expected {
                let orphan = actual
                    .difference(&expected)
                    .next()
                    .copied()
                    .unwrap_or("<missing>");
                return Err(ManifestError::InvalidObservableSchema {
                    family: family.to_string(),
                    reason: format!("orphan case requirement {orphan}"),
                });
            }
        }
        Ok(())
    }

    /// Return all compared cases in manifest order.  This is the single case
    /// registry consumed by focused adapter probes and completeness checks.
    pub fn behavior_cases(&self) -> Result<Vec<BehaviorCase>, ManifestError> {
        self.validate_observable_registry()?;
        Ok(self
            .entries
            .iter()
            .filter_map(|entry| {
                let case = entry.behavior_case.as_ref()?;
                if matches!(
                    entry.class,
                    ParityClass::Exact | ParityClass::SemanticMismatch
                ) {
                    Some(BehaviorCase {
                        behavior_case: case.clone(),
                        entry_id: entry.entry_id.clone(),
                        operation_family: entry.operation_family.clone(),
                        class: entry.class,
                        cli_ids: entry.cli_ids.clone(),
                        http_ids: entry.http_ids.clone(),
                    })
                } else {
                    None
                }
            })
            .collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorCase {
    pub behavior_case: String,
    pub entry_id: String,
    pub operation_family: String,
    pub class: ParityClass,
    pub cli_ids: Vec<String>,
    pub http_ids: Vec<String>,
}

/// Parse and validate the checked-in schemas and cases.
pub fn checked_registry() -> Result<(Manifest, Vec<BehaviorCase>), ManifestError> {
    let manifest = checked_manifest()?;
    manifest.validate_observable_registry()?;
    let cases = manifest.behavior_cases()?;
    Ok((manifest, cases))
}

fn validate_observable_schema(schema: &ObservableSchema) -> Result<(), ManifestError> {
    let family = schema.operation_family.trim();
    validate_schema_header(schema, family)?;
    validate_schema_normalizations(schema)?;
    validate_schema_metadata(schema, family)?;
    validate_case_requirements(schema, family)
}

fn validate_schema_header(schema: &ObservableSchema, family: &str) -> Result<(), ManifestError> {
    if family.is_empty() {
        return Err(invalid_schema(schema, "operation_family is empty"));
    }
    if schema.version != SCHEMA_VERSION {
        return Err(invalid_schema(
            schema,
            &format!("unsupported version {}", schema.version),
        ));
    }
    validate_semantic_fields(family, "required_fields", &schema.required_fields)
}
fn validate_schema_normalizations(schema: &ObservableSchema) -> Result<(), ManifestError> {
    let mut normalizations = BTreeSet::new();
    if schema
        .allowed_normalizations
        .iter()
        .any(|normalization| !normalizations.insert(*normalization))
    {
        return Err(invalid_schema(
            schema,
            "allowed_normalizations must be unique",
        ));
    }
    let has_timestamps = !schema.nondeterministic_fields.is_empty();
    let allows_timestamps = schema
        .allowed_normalizations
        .contains(&NormalizationRule::NondeterministicTimestamp);
    if has_timestamps != allows_timestamps {
        return Err(invalid_schema(
            schema,
            "nondeterministic_fields must match timestamp normalization",
        ));
    }
    if schema.success_cases.is_empty() || schema.error_cases.is_empty() {
        return Err(invalid_schema(
            schema,
            "success_cases and error_cases must be nonempty",
        ));
    }
    Ok(())
}

fn validate_schema_metadata(schema: &ObservableSchema, family: &str) -> Result<(), ManifestError> {
    if schema
        .ignored_fields
        .iter()
        .any(|field| !field.trim().to_ascii_lowercase().starts_with("transport."))
    {
        return Err(invalid_schema(
            schema,
            "only transport fields may be ignored",
        ));
    }
    let actors_valid = [
        ObservableActor::Authorized,
        ObservableActor::Unauthenticated,
        ObservableActor::Forbidden,
    ]
    .iter()
    .all(|actor| schema.actors.contains(actor));
    if schema.actors.is_empty() || !actors_valid {
        return Err(invalid_schema(
            schema,
            "actors must include authorized, unauthenticated and forbidden",
        ));
    }
    validate_observable_rules(schema, family)
}

fn validate_observable_rules(schema: &ObservableSchema, family: &str) -> Result<(), ManifestError> {
    let forbidden = ["auth", "errors", "redaction", "state", "retry"];
    let rules = [
        ("ordering", schema.ordering),
        ("pagination", schema.pagination),
        ("time", schema.time),
        ("auth", schema.auth),
        ("errors", schema.errors),
        ("redaction", schema.redaction),
        ("state", schema.state),
        ("retry", schema.retry),
    ];
    if let Some((name, _)) = rules
        .iter()
        .find(|(name, rule)| forbidden.contains(name) && *rule == ObservableRule::NotApplicable)
    {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{name} cannot be not-applicable"),
        });
    }
    Ok(())
}

fn validate_case_requirements(
    schema: &ObservableSchema,
    family: &str,
) -> Result<(), ManifestError> {
    if schema.case_requirements.is_empty() {
        return Err(invalid_schema(schema, "case_requirements must be nonempty"));
    }
    let mut cases = BTreeSet::new();
    for requirement in &schema.case_requirements {
        validate_case_requirement(requirement, family, &mut cases)?;
    }
    Ok(())
}

fn validate_case_requirement(
    requirement: &ObservableCaseRequirement,
    family: &str,
    cases: &mut BTreeSet<String>,
) -> Result<(), ManifestError> {
    if requirement.behavior_case.trim().is_empty()
        || !cases.insert(requirement.behavior_case.clone())
    {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: "case_requirements must have unique nonempty behavior_case values".into(),
        });
    }
    validate_semantic_fields(family, "case required_fields", &requirement.required_fields)?;
    validate_case_paths(
        requirement,
        family,
        &requirement.invariant_fields,
        "invariant",
    )?;
    validate_case_paths(
        requirement,
        family,
        &requirement.generated_id_fields,
        "generated ID",
    )
}

fn validate_case_paths(
    requirement: &ObservableCaseRequirement,
    family: &str,
    paths: &[String],
    label: &str,
) -> Result<(), ManifestError> {
    if let Some(path) = paths.iter().find(|path| {
        !requirement
            .required_fields
            .iter()
            .any(|field| field == *path)
    }) {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{label} {path} is not required"),
        });
    }
    Ok(())
}

fn invalid_schema(schema: &ObservableSchema, reason: &str) -> ManifestError {
    ManifestError::InvalidObservableSchema {
        family: schema.operation_family.clone(),
        reason: reason.into(),
    }
}

fn validate_semantic_fields(
    family: &str,
    label: &str,
    fields: &[String],
) -> Result<(), ManifestError> {
    if fields.is_empty() || fields.iter().any(|field| field.trim().is_empty()) {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{label} must be nonempty"),
        });
    }
    let mut seen = BTreeSet::new();
    let all_envelopes = fields.iter().all(|field| {
        matches!(
            field.trim().to_ascii_lowercase().as_str(),
            "ok" | "data" | "result" | "response" | "body"
        )
    });
    let has_duplicate = fields
        .iter()
        .map(|field| field.trim().to_ascii_lowercase())
        .any(|field| !seen.insert(field));
    if all_envelopes || has_duplicate {
        return Err(ManifestError::InvalidObservableSchema {
            family: family.into(),
            reason: format!("{label} must contain unique semantic paths"),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservableError {
    MissingField { side: &'static str, field: String },
    MissingCaseRequirement { case: String },
    InvalidInvariant { side: &'static str, field: String },
    Mismatch { field: String },
}

impl fmt::Display for ObservableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ObservableError {}

/// Compare observations using the exact semantic requirement for one case.
pub fn compare_observables_for_case(
    schema: &ObservableSchema,
    behavior_case: &str,
    cli: &serde_json::Value,
    http: &serde_json::Value,
) -> Result<(), ObservableError> {
    let (left, right) = validated_observations(schema, behavior_case, cli, http)?;
    if left != right {
        return Err(ObservableError::Mismatch {
            field: "<observable>".into(),
        });
    }
    Ok(())
}

/// Validate each side of a semantic-mismatch case without requiring equality.
pub fn validate_observables_for_case(
    schema: &ObservableSchema,
    behavior_case: &str,
    cli: &serde_json::Value,
    http: &serde_json::Value,
) -> Result<(), ObservableError> {
    validated_observations(schema, behavior_case, cli, http).map(|_| ())
}

fn validated_observations(
    schema: &ObservableSchema,
    behavior_case: &str,
    cli: &serde_json::Value,
    http: &serde_json::Value,
) -> Result<(serde_json::Value, serde_json::Value), ObservableError> {
    let requirement = schema
        .case_requirements
        .iter()
        .find(|requirement| requirement.behavior_case == behavior_case)
        .ok_or_else(|| ObservableError::MissingCaseRequirement {
            case: behavior_case.into(),
        })?;
    let required = &requirement.required_fields;
    let generated = &requirement.generated_id_fields;
    let invariants = &requirement.invariant_fields;
    let left = normalized_observation(schema, cli, required, generated, "cli")?;
    let right = normalized_observation(schema, http, required, generated, "http")?;
    validate_invariants(&left, invariants, "cli")?;
    validate_invariants(&right, invariants, "http")?;
    Ok((left, right))
}

fn normalized_observation(
    schema: &ObservableSchema,
    original: &serde_json::Value,
    required: &[String],
    generated: &[String],
    side: &'static str,
) -> Result<serde_json::Value, ObservableError> {
    let normalize_envelope = schema
        .allowed_normalizations
        .contains(&NormalizationRule::Envelope);
    let mut value = if normalize_envelope {
        strip_transport_envelope(original.clone())
    } else {
        original.clone()
    };
    for field in required {
        if lookup_observable(&value, field).is_none()
            && !(normalize_envelope && lookup_observable(original, field).is_some())
        {
            return Err(ObservableError::MissingField {
                side,
                field: field.clone(),
            });
        }
    }
    let timestamps: BTreeSet<_> = schema
        .nondeterministic_fields
        .iter()
        .map(String::as_str)
        .collect();
    let generated: BTreeSet<_> = if schema
        .allowed_normalizations
        .contains(&NormalizationRule::GeneratedId)
    {
        generated.iter().map(String::as_str).collect()
    } else {
        BTreeSet::new()
    };
    let mut ids = BTreeMap::new();
    canonicalize_observation(&mut value, "", &generated, &timestamps, &mut ids);
    Ok(value)
}

fn validate_invariants(
    value: &serde_json::Value,
    invariants: &[String],
    side: &'static str,
) -> Result<(), ObservableError> {
    for field in invariants {
        if lookup_observable(value, field) != Some(&serde_json::Value::Bool(true)) {
            return Err(ObservableError::InvalidInvariant {
                side,
                field: field.clone(),
            });
        }
    }
    Ok(())
}

fn lookup_observable<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn strip_transport_envelope(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(mut object) if object.len() == 1 => {
            for key in ["data", "result", "response", "body"] {
                if let Some(inner) = object.remove(key) {
                    return strip_transport_envelope(inner);
                }
            }
            serde_json::Value::Object(object)
        }
        other => other,
    }
}
struct Canonicalizer<'a> {
    generated: &'a BTreeSet<&'a str>,
    timestamps: &'a BTreeSet<&'a str>,
    ids: BTreeMap<String, String>,
}

impl<'a> Canonicalizer<'a> {
    fn visit(&mut self, value: &mut serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Object(object) => {
                for (key, child) in object {
                    let child_path = join_path(path, key);
                    self.visit(child, &child_path);
                }
            }
            serde_json::Value::Array(array) => {
                for (index, child) in array.iter_mut().enumerate() {
                    let child_path = join_path(path, &index.to_string());
                    self.visit(child, &child_path);
                }
            }
            serde_json::Value::String(text) => self.normalize_string(text, path),
            _ => {}
        }
    }

    fn normalize_string(&mut self, text: &mut String, path: &str) {
        if path_matches(self.timestamps, path) {
            *text = "<nondeterministic-timestamp>".into();
        } else if path_matches(self.generated, path) {
            let original = text.clone();
            let ordinal = self.ids.len() + 1;
            let canonical = self
                .ids
                .entry(original)
                .or_insert_with(|| format!("<generated-id-{ordinal}>"))
                .clone();
            *text = canonical;
        }
    }
}

fn canonicalize_observation(
    value: &mut serde_json::Value,
    path: &str,
    generated: &BTreeSet<&str>,
    timestamps: &BTreeSet<&str>,
    ids: &mut BTreeMap<String, String>,
) {
    let mut canonicalizer = Canonicalizer {
        generated,
        timestamps,
        ids: std::mem::take(ids),
    };
    canonicalizer.visit(value, path);
    *ids = canonicalizer.ids;
}

fn join_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.into()
    } else {
        format!("{parent}.{child}")
    }
}

fn path_matches(patterns: &BTreeSet<&str>, path: &str) -> bool {
    patterns.iter().any(|pattern| {
        let expected: Vec<_> = pattern.split('.').collect();
        let actual: Vec<_> = path.split('.').collect();
        expected.len() == actual.len()
            && expected
                .iter()
                .zip(actual)
                .all(|(expected, actual)| *expected == "*" || *expected == actual)
    })
}

fn validate_entry_identity(
    entry: &ParityEntry,
    seen_entries: &mut BTreeSet<String>,
    seen_anchors: &mut BTreeSet<String>,
) -> Result<(), ManifestError> {
    for (field, value) in [
        ("entry_id", entry.entry_id.as_str()),
        ("operation_family", entry.operation_family.as_str()),
        ("docs_anchor", entry.docs_anchor.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ManifestError::EmptyField {
                entry: entry.entry_id.clone(),
                field,
            });
        }
    }
    if !seen_entries.insert(entry.entry_id.clone()) {
        return Err(ManifestError::DuplicateEntry(entry.entry_id.clone()));
    }
    let anchor = entry.docs_anchor.trim_start_matches('#');
    if anchor.trim().is_empty() {
        return Err(ManifestError::EmptyField {
            entry: entry.entry_id.clone(),
            field: "docs_anchor",
        });
    }
    if !seen_anchors.insert(anchor.to_string()) {
        return Err(ManifestError::DuplicateAnchor(anchor.to_string()));
    }
    Ok(())
}

fn validate_surfaces(
    entry: &ParityEntry,
    side: &'static str,
    ids: &[String],
    inventory: &BTreeSet<&str>,
    seen: &mut BTreeSet<String>,
) -> Result<(), ManifestError> {
    let field = if side == "cli" { "cli_ids" } else { "http_ids" };
    for id in ids {
        if id.trim().is_empty() {
            return Err(ManifestError::EmptyField {
                entry: entry.entry_id.clone(),
                field,
            });
        }
        if id.contains('*') {
            return Err(ManifestError::WildcardSurface {
                entry: entry.entry_id.clone(),
                surface: id.clone(),
            });
        }
        if !inventory.contains(id.as_str()) {
            return Err(ManifestError::UnknownSurface {
                side: field,
                surface: id.clone(),
            });
        }
        if !seen.insert(id.clone()) {
            return Err(ManifestError::DuplicateSurface {
                surface: id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_entry_class(entry: &ParityEntry) -> Result<(), ManifestError> {
    let has_cli = !entry.cli_ids.is_empty();
    let has_http = !entry.http_ids.is_empty();
    let valid_sides = match entry.class {
        ParityClass::Exact | ParityClass::SemanticMismatch => has_cli && has_http,
        ParityClass::CliOnly => has_cli && !has_http,
        ParityClass::HttpOnly => !has_cli && has_http,
    };
    if !valid_sides {
        return Err(ManifestError::WrongClassSides {
            entry: entry.entry_id.clone(),
            class: entry.class,
        });
    }
    if matches!(
        entry.class,
        ParityClass::Exact | ParityClass::SemanticMismatch
    ) && entry
        .behavior_case
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return Err(ManifestError::MissingBehaviorCase(entry.entry_id.clone()));
    }
    Ok(())
}

fn validate_entry_metadata(entry: &ParityEntry) -> Result<(), ManifestError> {
    validate_semantic_metadata(entry)?;
    validate_adapter_metadata(entry)
}

fn validate_semantic_metadata(entry: &ParityEntry) -> Result<(), ManifestError> {
    match entry.class {
        ParityClass::SemanticMismatch if entry.semantic_difference.is_none() => Err(
            ManifestError::MissingSemanticDifference(entry.entry_id.clone()),
        ),
        ParityClass::Exact | ParityClass::CliOnly | ParityClass::HttpOnly
            if entry.semantic_difference.is_some() =>
        {
            Err(ManifestError::IncompatibleSemanticDifference(
                entry.entry_id.clone(),
            ))
        }
        _ => Ok(()),
    }
}

fn validate_adapter_metadata(entry: &ParityEntry) -> Result<(), ManifestError> {
    match entry.class {
        ParityClass::CliOnly | ParityClass::HttpOnly
            if entry
                .adapter_only
                .as_ref()
                .is_none_or(|r| r.auth.trim().is_empty() || r.lifecycle.trim().is_empty()) =>
        {
            Err(ManifestError::MissingAdapterRationale(
                entry.entry_id.clone(),
            ))
        }
        ParityClass::Exact | ParityClass::SemanticMismatch if entry.adapter_only.is_some() => Err(
            ManifestError::IncompatibleAdapterOnly(entry.entry_id.clone()),
        ),
        _ => Ok(()),
    }
}

fn validate_complete(
    side: &'static str,
    inventory: &BTreeSet<&str>,
    seen: &BTreeSet<String>,
) -> Result<(), ManifestError> {
    for id in inventory {
        if !seen.contains(*id) {
            return Err(ManifestError::MissingSurface {
                side,
                surface: id.to_string(),
            });
        }
    }
    Ok(())
}

/// Canonical HTTP IDs from the router's `(METHOD, path)` inventory.
pub fn http_ids(routes: &[(&str, &str)]) -> Vec<String> {
    routes
        .iter()
        .map(|(method, path)| {
            let method = method.trim().to_ascii_uppercase();
            let path = normalize_route(path);
            format!("{method} {path}")
        })
        .collect()
}

fn normalize_route(path: &str) -> String {
    let mut path = path.trim().to_string();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    path.split('/')
        .map(|segment| {
            segment
                .strip_prefix('*')
                .map_or_else(|| segment.to_string(), |name| format!(":{name}"))
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn render_markdown(manifest: &Manifest) -> String {
    let mut counts = BTreeMap::<ParityClass, usize>::new();
    for entry in &manifest.entries {
        *counts.entry(entry.class).or_default() += 1;
    }
    let mut entries = manifest.entries.clone();
    entries.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
    let mut output = String::from("# CLI / HTTP parity\n\n<!-- BEGIN GENERATED PARITY -->\n\n");
    output.push_str(&format!(
        "Manifest schema version: **{}**.\n\n",
        manifest.schema_version
    ));
    output.push_str("| Class | Entries |\n|---|---:|\n");
    for class in [
        ParityClass::Exact,
        ParityClass::SemanticMismatch,
        ParityClass::CliOnly,
        ParityClass::HttpOnly,
    ] {
        output.push_str(&format!(
            "| {} | {} |\n",
            class_name(class),
            counts.get(&class).copied().unwrap_or(0)
        ));
    }
    output.push_str("\n| Entry | Class | Operation family | CLI IDs | HTTP IDs | Behavior case |\n|---|---|---|---|---|---|\n");
    for entry in entries {
        let cli = entry.cli_ids.join("<br>");
        let http = entry.http_ids.join("<br>");
        let anchor = entry.docs_anchor.trim_start_matches('#');
        output.push_str(&format!(
            "| <a id=\"{anchor}\"></a>`{}` | {} | `{}` | {} | {} | {} |\n",
            entry.entry_id,
            class_name(entry.class),
            entry.operation_family,
            cli,
            http,
            entry.behavior_case.as_deref().unwrap_or("—")
        ));
        if let Some(diff) = entry.semantic_difference {
            output.push_str(&format!(
                "\n> `{}`: CLI — {}; HTTP — {}.\n\n",
                diff.kind, diff.cli_behavior, diff.http_behavior
            ));
        }
        if let Some(rationale) = entry.rationale {
            output.push_str(&format!("> Rationale: {}\n\n", rationale));
        }
    }
    output.push_str("\n<!-- END GENERATED PARITY -->\n");
    output
}

fn class_name(class: ParityClass) -> &'static str {
    match class {
        ParityClass::Exact => "exact",
        ParityClass::SemanticMismatch => "semantic-mismatch",
        ParityClass::CliOnly => "cli-only",
        ParityClass::HttpOnly => "http-only",
    }
}

pub fn check_docs_freshness(manifest: &Manifest, docs: &str) -> Result<(), ManifestError> {
    let generated = render_markdown(manifest);
    if normalize_generated_text(docs) != generated {
        return Err(ManifestError::Parse(
            "generated parity documentation is stale; regenerate from the manifest".into(),
        ));
    }
    for entry in &manifest.entries {
        let needle = format!("id=\"{}\"", entry.docs_anchor.trim_start_matches('#'));
        if !docs.contains(&needle) {
            return Err(ManifestError::DocsAnchorMissing {
                entry: entry.entry_id.clone(),
                anchor: entry.docs_anchor.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        super::check_docs_freshness(&manifest, include_str!("../docs/cli-http-parity.md")).unwrap();
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
                include_str!("../fixtures/cli-http-parity/duplicate-entry.toml"),
            ),
            (
                "duplicate-surface",
                include_str!("../fixtures/cli-http-parity/duplicate-surface.toml"),
            ),
            (
                "unknown-surface",
                include_str!("../fixtures/cli-http-parity/unknown-surface.toml"),
            ),
            (
                "empty-side",
                include_str!("../fixtures/cli-http-parity/empty-side.toml"),
            ),
            (
                "wildcard",
                include_str!("../fixtures/cli-http-parity/wildcard.toml"),
            ),
            (
                "wrong-class",
                include_str!("../fixtures/cli-http-parity/wrong-class.toml"),
            ),
            (
                "duplicate-anchor",
                include_str!("../fixtures/cli-http-parity/duplicate-anchor.toml"),
            ),
            (
                "incompatible-semantic",
                include_str!("../fixtures/cli-http-parity/incompatible-semantic.toml"),
            ),
            (
                "incompatible-adapter",
                include_str!("../fixtures/cli-http-parity/incompatible-adapter.toml"),
            ),
            (
                "unsupported-version",
                include_str!("../fixtures/cli-http-parity/unsupported-version.toml"),
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
                compare_observables_for_case(&schema, "fixture.success", &wrapped, &wrapped)
                    .is_ok()
            );
        }
        let schema = comparison_schema();
        let value = serde_json::json!({"status": "ok", "id": "same", "created_at": "now"});
        assert!(compare_observables_for_case(
            &schema,
            "fixture.success",
            &value,
            &serde_json::json!({"transport": true})
        )
        .is_err());
    }
}
