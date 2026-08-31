use omakure::operation_catalog::{
    self, CatalogError, DOCS_PATH, MANIFEST_PATH, SUPPORT_MATRIX_PATH,
};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("operation-catalog: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), CatalogError> {
    let root = PathBuf::from(env::var_os("MISE_PROJECT_ROOT").unwrap_or_else(|| ".".into()));
    let catalog = operation_catalog::validate_current()?;
    let command = env::args().nth(1).unwrap_or_else(|| "--write".into());
    let docs = operation_catalog::render_markdown(&catalog);
    let support_matrix = operation_catalog::render_support_matrix(&catalog);
    match command.as_str() {
        "--write" => {
            fs::write(root.join(DOCS_PATH), docs)
                .map_err(|error| CatalogError::Parse(error.to_string()))?;
            fs::write(root.join(SUPPORT_MATRIX_PATH), support_matrix)
                .map_err(|error| CatalogError::Parse(error.to_string()))?;
        }
        "--check" => {
            let actual_docs = fs::read_to_string(root.join(DOCS_PATH))
                .map_err(|error| CatalogError::Parse(error.to_string()))?;
            let actual_support = fs::read_to_string(root.join(SUPPORT_MATRIX_PATH))
                .map_err(|error| CatalogError::Parse(error.to_string()))?;
            operation_catalog::check_docs_freshness(&catalog, &actual_docs)?;
            operation_catalog::check_support_matrix_freshness(&catalog, &actual_support)?;
        }
        other => {
            return Err(CatalogError::Parse(format!(
                "unknown argument {other}; expected --write or --check"
            )))
        }
    }
    println!("operation catalog {command}: {MANIFEST_PATH}, {DOCS_PATH}, {SUPPORT_MATRIX_PATH}");
    Ok(())
}
