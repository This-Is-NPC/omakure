use super::*;
use crate::operations::core::args_contain_flag;
use crate::run_executor::{ExecutionResult, ExecutionTerminal};
use crate::runs::RunState;
use crate::test_support::workspace_in;
use rstest::rstest;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn write_schema_script(tmp: &TempDir, name: &str, schema_json: &str, body: &str) -> PathBuf {
    let path = tmp.path().join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let contents = format!(
        "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {}\n# OMAKURE_SCHEMA_END\n{}\n",
        schema_json, body
    );
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&path)
            .unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, contents).unwrap();
    }
    path
}

fn inline_row(workspace: &Workspace, script: &Path) -> crate::runs::RunRow {
    let conn = runs::open(workspace).unwrap();
    runs::start_inline(
        &conn,
        script.to_string_lossy().as_ref(),
        &[],
        "inline:test",
        EnqueueOptions {
            run_id: Some("rid-inline".into()),
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[cfg(unix)]
mod environment;
mod lifecycle;
mod path_and_fields;
#[cfg(unix)]
mod secrets;

#[cfg(unix)]
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(unix)]
fn read_all_bytes_under(dir: &Path) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Ok(bytes) = fs::read(&path) {
                    buf.extend_from_slice(&bytes);
                }
            } else if path.is_dir() {
                buf.extend_from_slice(&read_all_bytes_under(&path));
            }
        }
    }
    buf
}
