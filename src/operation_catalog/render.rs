use super::*;
use crate::inventory::normalize_generated_text;
use serde::Serialize;

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
    if normalize_generated_text(docs) != render_markdown(catalog) {
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
    if normalize_generated_text(support_matrix) != render_support_matrix(catalog) {
        return Err(CatalogError::Parse(
            "generated operation support matrix is stale; regenerate from the catalog".into(),
        ));
    }
    Ok(())
}
