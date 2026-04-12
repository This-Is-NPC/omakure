use crate::domain::{extract_schema_block, parse_schema, Schema};
use crate::error::{AppResult, ScriptError};
use crate::ports::{ScriptRepository, WorkspaceEntry, WorkspaceEntryKind};
use crate::runtime::{script_kind, ScriptKind};

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::util::read_dir_or_empty;
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
        let entries = read_dir_or_empty(dir)?;

        for entry in entries {
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

    fn read_schema(&self, script: &Path) -> AppResult<Schema> {
        let prefixes = match script_kind(script) {
            Some(ScriptKind::Bash) => vec!["#"],
            Some(ScriptKind::PowerShell) => vec!["#", ";"],
            Some(ScriptKind::Python) => vec!["#"],
            None => return Err(ScriptError::UnsupportedType.into()),
        };

        let contents = fs::read_to_string(script)?;
        let block = extract_schema_block(&contents, &prefixes)?;
        Ok(parse_schema(&block)?)
    }
}

fn collect_scripts(dir: &Path, scripts: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = read_dir_or_empty(dir)?;

    for entry in entries {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use std::fs;
    use tempfile::TempDir;

    #[fixture]
    fn workspace_with_scripts() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        // Top-level scripts
        fs::write(root.join("deploy.sh"), "#!/bin/bash\necho deploy").unwrap();
        fs::write(root.join("setup.py"), "print('setup')").unwrap();
        fs::write(root.join("readme.txt"), "not a script").unwrap();

        // Subdirectory with scripts
        fs::create_dir_all(root.join("infra")).unwrap();
        fs::write(root.join("infra/provision.bash"), "#!/bin/bash").unwrap();
        fs::write(root.join("infra/config.ps1"), "Write-Host hi").unwrap();

        // Hidden dirs that should be skipped
        fs::create_dir_all(root.join(".history")).unwrap();
        fs::write(root.join(".history/old.sh"), "#!/bin/bash").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/hook.sh"), "#!/bin/bash").unwrap();
        fs::create_dir_all(root.join(".omaken/envs")).unwrap();
        fs::write(root.join(".omaken/envs/dev.conf"), "KEY=val").unwrap();

        (tmp, root)
    }

    #[rstest]
    fn test_list_entries_sorts_dirs_first_then_alpha(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        let repo = FsWorkspaceRepository::new(&root);
        let entries = repo.list_entries(&root).unwrap();

        // Directories come first
        let kinds: Vec<_> = entries.iter().map(|e| e.kind).collect();
        let first_script_idx = kinds
            .iter()
            .position(|k| *k == WorkspaceEntryKind::Script)
            .unwrap_or(kinds.len());
        let last_dir_idx = kinds
            .iter()
            .rposition(|k| *k == WorkspaceEntryKind::Directory)
            .unwrap_or(0);
        assert!(
            last_dir_idx < first_script_idx,
            "dirs must come before scripts"
        );
    }

    #[rstest]
    fn test_list_entries_skips_hidden_dirs(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        let repo = FsWorkspaceRepository::new(&root);
        let entries = repo.list_entries(&root).unwrap();

        let names: Vec<String> = entries
            .iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(!names.contains(&".history".to_string()));
        assert!(!names.contains(&".git".to_string()));
    }

    #[rstest]
    fn test_list_entries_filters_extensions(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        let repo = FsWorkspaceRepository::new(&root);
        let entries = repo.list_entries(&root).unwrap();

        let script_names: Vec<String> = entries
            .iter()
            .filter(|e| e.kind == WorkspaceEntryKind::Script)
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(script_names.contains(&"deploy.sh".to_string()));
        assert!(script_names.contains(&"setup.py".to_string()));
        assert!(
            !script_names.contains(&"readme.txt".to_string()),
            "txt files should be excluded"
        );
    }

    #[rstest]
    fn test_list_scripts_recursive(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        let repo = FsWorkspaceRepository::new(&root);
        let scripts = repo.list_scripts_recursive().unwrap();

        let names: Vec<String> = scripts
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"deploy.sh".to_string()));
        assert!(names.contains(&"setup.py".to_string()));
        assert!(names.contains(&"provision.bash".to_string()));
        assert!(names.contains(&"config.ps1".to_string()));
        assert_eq!(names.len(), 4);
    }

    #[rstest]
    fn test_list_scripts_recursive_skips_hidden(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        let repo = FsWorkspaceRepository::new(&root);
        let scripts = repo.list_scripts_recursive().unwrap();

        for script in &scripts {
            let path_str = script.to_string_lossy();
            assert!(!path_str.contains(".history"), "should skip .history");
            assert!(!path_str.contains(".git/"), "should skip .git");
            assert!(
                !path_str.contains(".omaken/envs"),
                "should skip .omaken/envs"
            );
        }
    }

    #[test]
    fn test_read_schema_valid() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.sh");
        fs::write(
            &script,
            r#"#!/bin/bash
# OMAKURE_SCHEMA_START
# {"Name": "Test Script", "Description": "A test", "Fields": []}
# OMAKURE_SCHEMA_END
echo "hello"
"#,
        )
        .unwrap();

        let repo = FsWorkspaceRepository::new(tmp.path());
        let schema = repo.read_schema(&script).unwrap();
        assert_eq!(schema.name, "Test Script");
        assert_eq!(schema.description, Some("A test".to_string()));
    }

    #[test]
    fn test_read_schema_no_block() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("bare.sh");
        fs::write(&script, "#!/bin/bash\necho hello").unwrap();

        let repo = FsWorkspaceRepository::new(tmp.path());
        let result = repo.read_schema(&script);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_entries_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let entries = repo.list_entries(tmp.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_should_skip_dir_omaken_envs() {
        let tmp = TempDir::new().unwrap();
        let omaken = tmp.path().join(".omaken");
        let envs = omaken.join("envs");
        fs::create_dir_all(&envs).unwrap();
        assert!(should_skip_dir(&envs));
    }

    #[test]
    fn test_should_skip_dir_regular_envs_not_skipped() {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        assert!(!should_skip_dir(&envs));
    }
}
