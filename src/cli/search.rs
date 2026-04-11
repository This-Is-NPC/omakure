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

    if json_output {
        let entries: Vec<ScriptListEntry> = results
            .into_iter()
            .map(|r| to_entry(r, workspace.root()))
            .collect();
        json::print_ok(entries);
        return Ok(());
    }

    if results.is_empty() {
        println!("(no matches)");
        return Ok(());
    }
    for r in results {
        let path = r.script_path.to_string_lossy();
        let desc = r.description.as_deref().unwrap_or("");
        if desc.is_empty() {
            println!(" - {}", path);
        } else {
            println!(" - {} — {}", path, desc);
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
