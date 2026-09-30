use super::model::{Manifest, ManifestError, ParityClass};
use crate::cli::inventory::normalize_generated_text;
use std::collections::BTreeMap;

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
