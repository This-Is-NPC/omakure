//! `omakure search <query>` — surface the SQLite-backed script index.

use crate::cli::args::SearchArgs;
use crate::cli::json;
use crate::cli::list::ScriptListEntry;
use crate::search_index::{SearchIndex, SearchResult};
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

pub fn run(
    scripts_dir: PathBuf,
    options: SearchArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let index = SearchIndex::new(workspace.search_db_path());
    // The search index normally rebuilds in the background from the TUI.
    // For one-shot CLI use we trigger the rebuild and block on it so the
    // results are always fresh, even on a workspace that has never opened
    // the TUI.
    index.start_background_rebuild(workspace.root().to_path_buf());
    block_until_ready(&index);

    let results = match index.query(&options.query) {
        Ok(results) => results,
        Err(err) => {
            if json_output {
                json::print_err(crate::cli::json::codes::INTERNAL, err.clone());
                std::process::exit(1);
            }
            return Err(err.into());
        }
    };

    let entries: Vec<ScriptListEntry> = results
        .into_iter()
        .map(|r| to_entry(r, workspace.root()))
        .filter(|entry| crate::cli::list::matches_all_tags(entry, &options.tag))
        .collect();

    if json_output {
        json::print_ok(entries);
        return Ok(());
    }

    if entries.is_empty() {
        println!("(no matches)");
        return Ok(());
    }
    for entry in entries {
        let desc = entry.description.as_deref().unwrap_or("");
        if desc.is_empty() {
            println!(" - {}", entry.relative_path);
        } else {
            println!(" - {} — {}", entry.relative_path, desc);
        }
    }
    Ok(())
}

fn to_entry(r: SearchResult, root: &std::path::Path) -> ScriptListEntry {
    // The search index stores workspace-relative paths. Join with the
    // root before canonicalizing so the JSON `absolute_path` is always a
    // real absolute filesystem path.
    let joined = if r.script_path.is_absolute() {
        r.script_path.clone()
    } else {
        root.join(&r.script_path)
    };
    let absolute_path = std::fs::canonicalize(&joined)
        .unwrap_or(joined)
        .to_string_lossy()
        .to_string();
    let relative_path = r.script_path.to_string_lossy().to_string();
    ScriptListEntry {
        absolute_path,
        relative_path,
        name: Some(r.display_name),
        description: r.description,
        tags: r.tags,
        field_count: 0,
        schema_error: r.schema_error,
    }
}

fn block_until_ready(index: &SearchIndex) {
    use crate::search_index::SearchStatus;
    use std::thread;
    use std::time::Duration;
    // Bounded wait — the index rebuild for a typical workspace finishes in
    // milliseconds. We cap at ~5s so a wedged background thread cannot
    // hang the CLI.
    for _ in 0..50 {
        match index.status() {
            SearchStatus::Ready { .. } | SearchStatus::Error(_) => return,
            _ => thread::sleep(Duration::from_millis(100)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search_index::SearchStatus;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn result(path: PathBuf, display_name: &str) -> SearchResult {
        SearchResult {
            script_path: path,
            display_name: display_name.to_string(),
            description: Some("Deploy app".to_string()),
            tags: vec!["ops".to_string()],
            schema_error: None,
        }
    }

    #[test]
    fn to_entry_joins_relative_path_and_canonicalizes() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("deploy.sh");
        std::fs::write(&script, "#!/usr/bin/env bash\n").unwrap();

        let entry = to_entry(result(PathBuf::from("deploy.sh"), "Deploy"), tmp.path());

        assert_eq!(entry.relative_path, "deploy.sh");
        assert_eq!(entry.absolute_path, script.to_string_lossy().to_string());
        assert_eq!(entry.name, Some("Deploy".to_string()));
        assert_eq!(entry.description, Some("Deploy app".to_string()));
        assert_eq!(entry.tags, vec!["ops".to_string()]);
    }

    #[test]
    fn to_entry_preserves_absolute_path_when_input_is_absolute() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("deploy.sh");
        std::fs::write(&script, "#!/usr/bin/env bash\n").unwrap();

        let entry = to_entry(result(script.clone(), "Deploy"), tmp.path());

        assert_eq!(entry.relative_path, script.to_string_lossy().to_string());
        assert_eq!(entry.absolute_path, script.to_string_lossy().to_string());
    }

    #[test]
    fn to_entry_falls_back_to_joined_path_when_target_is_missing() {
        let tmp = TempDir::new().unwrap();
        let joined = tmp.path().join("missing.sh");

        let entry = to_entry(result(PathBuf::from("missing.sh"), "Missing"), tmp.path());

        assert_eq!(entry.absolute_path, joined.to_string_lossy().to_string());
        assert_eq!(entry.relative_path, "missing.sh");
    }

    #[test]
    fn block_until_ready_stops_when_index_reports_ready() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("deploy.sh");
        std::fs::write(&script, "#!/usr/bin/env bash\n").unwrap();
        let db_path = tmp.path().join("search.sqlite");
        let index = SearchIndex::new(db_path);

        index.start_background_rebuild(tmp.path().to_path_buf());
        block_until_ready(&index);

        assert_eq!(index.status(), SearchStatus::Ready { script_count: 1 });
    }

    #[test]
    fn run_human_format_no_matches() {
        let tmp = TempDir::new().unwrap();
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
        let tmp = TempDir::new().unwrap();
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
    fn run_human_format_with_results_and_descriptions() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("deploy.sh"),
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\":\"Deploy\",\"Description\":\"Ship\",\"Fields\":[]}\n# OMAKURE_SCHEMA_END\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("bare.sh"), "#!/usr/bin/env bash\n").unwrap();
        run(
            tmp.path().to_path_buf(),
            SearchArgs {
                query: String::new(),
                tag: vec![],
            },
            false,
        )
        .unwrap();
    }

    #[test]
    fn block_until_ready_stops_when_index_reports_error() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().to_path_buf();
        let index = SearchIndex::new(db_path);

        index.start_background_rebuild(tmp.path().to_path_buf());
        block_until_ready(&index);

        assert!(matches!(index.status(), SearchStatus::Error(_)));
    }
}
