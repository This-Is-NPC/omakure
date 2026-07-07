//! `omakure search <query>` — surface the SQLite-backed script index.

use crate::cli::args::SearchArgs;
use crate::cli::json::{self, codes};
use crate::cli::list::ScriptListEntry;
use crate::operations::search::{self, SearchScriptsRequest};
use crate::operations::{OperationError, OperationErrorCode};
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

pub fn run(
    scripts_dir: PathBuf,
    options: SearchArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    let entries = match search::search_scripts(
        &workspace,
        SearchScriptsRequest {
            query: options.query,
            tags: options.tag,
            refresh: true,
        },
    ) {
        Ok(entries) => entries,
        Err(err) => return emit_operation_error(json_output, err),
    };

    if json_output {
        json::print_ok(entries);
        return Ok(());
    }

    if entries.is_empty() {
        println!("(no matches)");
        return Ok(());
    }
    for entry in entries {
        print_entry(&entry);
    }
    Ok(())
}

fn print_entry(entry: &ScriptListEntry) {
    let desc = entry.description.as_deref().unwrap_or("");
    if desc.is_empty() {
        println!(" - {}", entry.relative_path);
    } else {
        println!(" - {} — {}", entry.relative_path, desc);
    }
}

fn emit_operation_error(json_output: bool, err: OperationError) -> Result<(), Box<dyn Error>> {
    let code = match err.code {
        OperationErrorCode::InvalidInput => codes::INVALID_ARGUMENT,
        _ => codes::INTERNAL,
    };
    if json_output {
        json::print_err(code, err.message);
        std::process::exit(1);
    }
    Err(err.message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn print_entry_accepts_entries_with_and_without_descriptions() {
        print_entry(&ScriptListEntry {
            absolute_path: "/tmp/deploy.sh".into(),
            relative_path: "deploy.sh".into(),
            name: Some("Deploy".into()),
            description: Some("Ship app".into()),
            tags: vec!["ops".into()],
            field_count: 0,
            schema_error: None,
        });
        print_entry(&ScriptListEntry {
            absolute_path: "/tmp/bare.sh".into(),
            relative_path: "bare.sh".into(),
            name: Some("Bare".into()),
            description: None,
            tags: Vec::new(),
            field_count: 0,
            schema_error: None,
        });
    }

    #[test]
    fn run_human_format_no_matches() {
        let tmp = tempfile::TempDir::new().unwrap();
        run(
            tmp.path().to_path_buf(),
            SearchArgs {
                query: "nothing".into(),
                tag: vec![],
            },
            false,
        )
        .unwrap();
    }

    #[test]
    fn run_json_format_with_results() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("deploy.sh"),
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\":\"Deploy\",\"Description\":\"Ship\",\"Tags\":[\"ops\"],\"Fields\":[]}\n# OMAKURE_SCHEMA_END\n",
        )
        .unwrap();
        run(
            tmp.path().to_path_buf(),
            SearchArgs {
                query: "deploy".into(),
                tag: vec!["ops".into()],
            },
            true,
        )
        .unwrap();
    }

    #[test]
    fn operation_result_preserves_script_list_shape() {
        let entry = ScriptListEntry {
            absolute_path: "/tmp/deploy.sh".into(),
            relative_path: "deploy.sh".into(),
            name: Some("Deploy".into()),
            description: Some("Ship app".into()),
            tags: vec!["ops".into()],
            field_count: 0,
            schema_error: None,
        };

        assert_eq!(entry.relative_path, "deploy.sh");
    }
}
