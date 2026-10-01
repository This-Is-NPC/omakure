use super::SCHEMA_VERSION;
use super::model::{
    InventorySets, Manifest, ManifestError, ParityClass, ParityEntry, SurfaceInventory,
};
use super::schema::{BehaviorCase, validate_observable_schema};
use std::collections::{BTreeMap, BTreeSet};

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
                .strip_prefix("{*")
                .and_then(|name| name.strip_suffix('}'))
                .or_else(|| {
                    segment
                        .strip_prefix('{')
                        .and_then(|name| name.strip_suffix('}'))
                })
                .map_or_else(|| segment.to_string(), |name| format!(":{name}"))
        })
        .collect::<Vec<_>>()
        .join("/")
}
