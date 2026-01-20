use crate::domain::{extract_schema_block, parse_schema, Schema};
use crate::ports::{ScriptRepository, WorkspaceEntry, WorkspaceEntryKind};
use crate::runtime::{script_kind, ScriptKind};

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub struct FsWorkspaceRepository {
    root: PathBuf,
}

impl FsWorkspaceRepository {
    pub fn new<P: Into<PathBuf>>(root: P) -> Self {
        Self { root: root.into() }
    }
}

impl ScriptRepository for FsWorkspaceRepository {
    fn list_entries(&self, dir: &Path) -> io::Result<Vec<WorkspaceEntry>> {
        let mut entries_out = Vec::new();
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                if err.kind() == io::ErrorKind::NotFound {
                    return Ok(entries_out);
                }
                return Err(err);
            }
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if should_skip_dir(&path) {
                    continue;
                }
                entries_out.push(WorkspaceEntry {
                    path,
                    kind: WorkspaceEntryKind::Directory,
                });
                continue;
            }
            if path.is_file() && script_kind(&path).is_some() {
                entries_out.push(WorkspaceEntry {
                    path,
                    kind: WorkspaceEntryKind::Script,
                });
            }
        }

        entries_out.sort_by(|a, b| match (a.kind, b.kind) {
            (WorkspaceEntryKind::Directory, WorkspaceEntryKind::Script) => std::cmp::Ordering::Less,
            (WorkspaceEntryKind::Script, WorkspaceEntryKind::Directory) => {
                std::cmp::Ordering::Greater
            }
            _ => entry_name(&a.path).cmp(&entry_name(&b.path)),
        });

        Ok(entries_out)
    }

    fn list_scripts_recursive(&self) -> io::Result<Vec<PathBuf>> {
        let mut scripts = Vec::new();
        collect_scripts(&self.root, &mut scripts)?;
        Ok(scripts)
    }

    fn read_schema(&self, script: &Path) -> Result<Schema, Box<dyn Error>> {
        let prefixes = match script_kind(script).ok_or("Unsupported script type")? {
            ScriptKind::Bash => vec!["#"],
            ScriptKind::PowerShell => vec!["#", ";"],
            ScriptKind::Python => vec!["#"],
        };

        let contents = fs::read_to_string(script)?;
        let block = extract_schema_block(&contents, &prefixes)
            .map_err(|err| format!("Schema block not found: {}", err))?;
        parse_schema(&block)
    }
}

fn collect_scripts(dir: &Path, scripts: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                return Ok(());
            }
            return Err(err);
        }
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_scripts(&path, scripts)?;
        } else if path.is_file() && script_kind(&path).is_some() {
            scripts.push(path);
        }
    }

    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str());
    if matches!(name, Some(".history") | Some(".git")) {
        return true;
    }
    if matches!(name, Some("envs")) {
        if let Some(parent) = path.parent().and_then(|parent| parent.file_name()) {
            if parent == ".omaken" {
                return true;
            }
        }
    }
    false
}

fn entry_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}
