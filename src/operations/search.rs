use crate::operations::core::ScriptSummary;
use crate::search_index::{SearchIndex, SearchResult, SearchStatus};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::thread;
use std::time::Duration;

use super::{OperationError, OperationErrorCode, OperationResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchScriptsRequest {
    pub query: String,
    pub tags: Vec<String>,
    pub refresh: bool,
}

pub fn search_scripts(
    workspace: &Workspace,
    request: SearchScriptsRequest,
) -> OperationResult<Vec<ScriptSummary>> {
    workspace.ensure_layout().map_err(io_error)?;

    let index = SearchIndex::new(workspace.search_db_path());
    if request.refresh {
        index.start_background_rebuild(workspace.scripts_root().to_path_buf());
        block_until_ready(&index);
    }

    let results = index
        .query(&request.query)
        .map_err(|err| OperationError::new(OperationErrorCode::IoFailed, err))?;

    Ok(results
        .into_iter()
        .map(|result| to_summary(result, workspace.scripts_root()))
        .filter(|entry| matches_all_tags(entry, &request.tags))
        .collect())
}

fn to_summary(result: SearchResult, root: &std::path::Path) -> ScriptSummary {
    let joined = if result.script_path.is_absolute() {
        result.script_path.clone()
    } else {
        root.join(&result.script_path)
    };
    let absolute_path = std::fs::canonicalize(&joined)
        .unwrap_or(joined)
        .to_string_lossy()
        .to_string();
    let relative_path = result.script_path.to_string_lossy().to_string();
    ScriptSummary {
        absolute_path,
        relative_path,
        name: Some(result.display_name),
        description: result.description,
        tags: result.tags,
        field_count: 0,
        schema_error: result.schema_error,
    }
}

fn matches_all_tags(entry: &ScriptSummary, required: &[String]) -> bool {
    required
        .iter()
        .all(|tag| entry.tags.iter().any(|entry_tag| entry_tag == tag))
}

fn block_until_ready(index: &SearchIndex) {
    for _ in 0..50 {
        match index.status() {
            SearchStatus::Ready { .. } | SearchStatus::Error(_) => return,
            _ => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn io_error(err: impl std::error::Error) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_script(dir: &std::path::Path, name: &str, tags: &[&str]) {
        let tags_json = if tags.is_empty() {
            String::new()
        } else {
            format!(
                ",\"Tags\":[{}]",
                tags.iter()
                    .map(|tag| format!("\"{tag}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        std::fs::write(
            dir.join(name),
            format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {{\"Name\":\"{name}\",\"Description\":\"Ship\",\"Fields\":[]{tags_json}}}\n# OMAKURE_SCHEMA_END\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn search_scripts_refreshes_index_and_filters_tags() {
        let dir = TempDir::new().unwrap();
        let workspace = Workspace::new(dir.path().to_path_buf());
        write_script(workspace.scripts_root(), "deploy.sh", &["ops"]);
        write_script(workspace.scripts_root(), "noise.sh", &["other"]);

        let results = search_scripts(
            &workspace,
            SearchScriptsRequest {
                query: "deploy".into(),
                tags: vec!["ops".into()],
                refresh: true,
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].relative_path, "deploy.sh");
    }

    #[test]
    fn search_scripts_empty_query_returns_indexed_scripts() {
        let dir = TempDir::new().unwrap();
        let workspace = Workspace::new(dir.path().to_path_buf());
        write_script(workspace.scripts_root(), "deploy.sh", &[]);

        let results = search_scripts(
            &workspace,
            SearchScriptsRequest {
                query: String::new(),
                tags: Vec::new(),
                refresh: true,
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
    }
}
