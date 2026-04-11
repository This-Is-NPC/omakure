use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::cli::json;
use crate::ports::ScriptRepository;
use serde::Serialize;
use std::error::Error;
use std::path::PathBuf;

/// JSON shape for one script in `omakure scripts --json`.
///
/// This is the same shape used by `omakure search --json` so an agent can
/// pipe results between the two commands without translating fields.
#[derive(Debug, Serialize)]
pub struct ScriptListEntry {
    pub absolute_path: String,
    pub relative_path: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub field_count: usize,
    pub schema_error: Option<String>,
}

pub fn run(scripts_dir: PathBuf, json_output: bool) -> Result<(), Box<dyn Error>> {
    let repo = FsWorkspaceRepository::new(scripts_dir.clone());
    let mut scripts = repo.list_scripts_recursive()?;
    scripts.sort();

    if json_output {
        let entries: Vec<ScriptListEntry> = scripts
            .into_iter()
            .map(|script| build_entry(&repo, &scripts_dir, script))
            .collect();
        json::print_ok(entries);
        return Ok(());
    }

    println!("Scripts folder: {}", scripts_dir.display());
    if scripts.is_empty() {
        println!("(no scripts found)");
        return Ok(());
    }

    for script in scripts {
        let display_path = script
            .strip_prefix(&scripts_dir)
            .unwrap_or(&script)
            .to_string_lossy();
        println!(" - {}", display_path);
    }

    Ok(())
}

fn build_entry(
    repo: &FsWorkspaceRepository,
    root: &std::path::Path,
    script: PathBuf,
) -> ScriptListEntry {
    let relative_path = script
        .strip_prefix(root)
        .unwrap_or(&script)
        .to_string_lossy()
        .to_string();
    let absolute_path = std::fs::canonicalize(&script)
        .unwrap_or_else(|_| script.clone())
        .to_string_lossy()
        .to_string();

    match repo.read_schema(&script) {
        Ok(schema) => ScriptListEntry {
            absolute_path,
            relative_path,
            name: Some(schema.name),
            description: schema.description,
            tags: schema.tags.unwrap_or_default(),
            field_count: schema.fields.len(),
            schema_error: None,
        },
        Err(err) => ScriptListEntry {
            absolute_path,
            relative_path,
            name: None,
            description: None,
            tags: Vec::new(),
            field_count: 0,
            schema_error: Some(err.to_string()),
        },
    }
}
