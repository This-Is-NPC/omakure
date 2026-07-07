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
        let ignore = IgnoreContext::load_for_dir(&self.root, dir);

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                if should_skip_dir(&path) || ignore.matches(&path, true) {
                    continue;
                }
                entries_out.push(WorkspaceEntry {
                    path,
                    kind: WorkspaceEntryKind::Directory,
                });
                continue;
            }
            if path.is_file() && script_kind(&path).is_some() && !ignore.matches(&path, false) {
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
        let mut ignore = IgnoreContext::load_for_dir(&self.root, &self.root);
        collect_scripts(&self.root, &mut ignore, &mut scripts)?;
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
        let mut schema = parse_schema(&block)?;
        schema.normalize_field_orders();
        Ok(schema)
    }
}

fn collect_scripts(
    dir: &Path,
    ignore: &mut IgnoreContext,
    scripts: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let entries = read_dir_or_empty(dir)?;

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if should_skip_dir(&path) || ignore.matches(&path, true) {
                continue;
            }
            let pushed = ignore.push_dir(&path);
            let result = collect_scripts(&path, ignore, scripts);
            if pushed {
                ignore.pop_dir();
            }
            result?;
        } else if path.is_file() && script_kind(&path).is_some() && !ignore.matches(&path, false) {
            scripts.push(path);
        }
    }

    Ok(())
}

#[derive(Debug, Default)]
struct IgnoreContext {
    files: Vec<OmakureIgnore>,
}

impl IgnoreContext {
    fn load_for_dir(root: &Path, dir: &Path) -> Self {
        let mut context = Self::default();
        let Ok(rel) = dir.strip_prefix(root) else {
            context.push_dir(root);
            return context;
        };

        context.push_dir(root);
        let mut cursor = root.to_path_buf();
        for component in rel.components() {
            let std::path::Component::Normal(part) = component else {
                continue;
            };
            cursor.push(part);
            context.push_dir(&cursor);
        }
        context
    }

    fn push_dir(&mut self, dir: &Path) -> bool {
        let ignore = OmakureIgnore::load(dir);
        if !ignore.patterns.is_empty() {
            self.files.push(ignore);
            return true;
        }
        false
    }

    fn pop_dir(&mut self) {
        self.files.pop();
    }

    fn matches(&self, path: &Path, is_dir: bool) -> bool {
        self.files.iter().any(|ignore| ignore.matches(path, is_dir))
    }
}

#[derive(Debug, Default)]
struct OmakureIgnore {
    root: PathBuf,
    patterns: Vec<IgnorePattern>,
}

#[derive(Debug)]
struct IgnorePattern {
    pattern: String,
    anchored: bool,
    dir_only: bool,
    has_slash: bool,
}

impl OmakureIgnore {
    fn load(root: &Path) -> Self {
        let path = root.join(".omakureignore");
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Self {
                    root: root.to_path_buf(),
                    patterns: Vec::new(),
                };
            }
            Err(err) => {
                eprintln!(
                    "warning: failed to read {}: {}; continuing without .omakureignore",
                    path.display(),
                    err
                );
                return Self {
                    root: root.to_path_buf(),
                    patterns: Vec::new(),
                };
            }
        };

        let patterns = contents
            .lines()
            .filter_map(|line| IgnorePattern::parse(line, &path))
            .collect();

        Self {
            root: root.to_path_buf(),
            patterns,
        }
    }

    fn matches(&self, path: &Path, is_dir: bool) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        let rel = normalize_relative_path(rel);
        if rel.is_empty() {
            return false;
        }
        self.patterns
            .iter()
            .any(|pattern| pattern.matches(&rel, is_dir))
    }
}

impl IgnorePattern {
    fn parse(line: &str, source: &Path) -> Option<Self> {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        if trimmed.contains('\0') {
            eprintln!(
                "warning: ignoring malformed .omakureignore pattern in {}",
                source.display()
            );
            return None;
        }
        let anchored = trimmed.starts_with('/');
        let dir_only = trimmed.ends_with('/');
        let pattern = trimmed
            .trim_start_matches('/')
            .trim_end_matches('/')
            .replace('\\', "/");
        if pattern.is_empty() {
            return None;
        }
        Some(Self {
            anchored,
            has_slash: pattern.contains('/'),
            pattern,
            dir_only,
        })
    }

    fn matches(&self, rel: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        if self.anchored || self.has_slash {
            wildcard_match(&self.pattern, rel)
        } else {
            rel.split('/')
                .any(|component| wildcard_match(&self.pattern, component))
        }
    }
}

fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut p, mut v) = (0, 0);
    let mut star = None;
    let mut star_value = 0;

    while v < value.len() {
        if p < pattern.len() && pattern[p] == value[v] {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            star_value = v;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            star_value += 1;
            v = star_value;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn should_skip_dir(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str());
    if matches!(name, Some(".history") | Some(".git") | Some(".omakure")) {
        return true;
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
        fs::create_dir_all(root.join(".omakure/envs")).unwrap();
        fs::write(root.join(".omakure/envs/dev.conf"), "KEY=val").unwrap();

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
            assert!(!path_str.contains(".omakure"), "should skip .omakure");
        }
    }

    #[rstest]
    fn test_omakureignore_excludes_matching_file(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        fs::write(root.join(".omakureignore"), "setup.py\n").unwrap();
        let repo = FsWorkspaceRepository::new(&root);

        let scripts = repo.list_scripts_recursive().unwrap();
        let names: Vec<String> = scripts
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert!(!names.contains(&"setup.py".to_string()));
        assert!(names.contains(&"deploy.sh".to_string()));
    }

    #[rstest]
    fn test_omakureignore_prunes_directory_subtree(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        fs::write(root.join(".omakureignore"), "infra/\n").unwrap();
        let repo = FsWorkspaceRepository::new(&root);

        let scripts = repo.list_scripts_recursive().unwrap();
        let paths: Vec<String> = scripts
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(!paths.iter().any(|path| path.starts_with("infra/")));
        assert!(paths.contains(&"deploy.sh".to_string()));
    }

    #[rstest]
    fn test_omakureignore_comments_blank_lines_and_globs(
        workspace_with_scripts: (TempDir, PathBuf),
    ) {
        let (_tmp, root) = workspace_with_scripts;
        fs::write(
            root.join(".omakureignore"),
            "# ignored patterns\n\ninfra/*.ps1\n*.py\n",
        )
        .unwrap();
        let repo = FsWorkspaceRepository::new(&root);

        let scripts = repo.list_scripts_recursive().unwrap();
        let paths: Vec<String> = scripts
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(!paths.contains(&"setup.py".to_string()));
        assert!(!paths.contains(&"infra/config.ps1".to_string()));
        assert!(paths.contains(&"infra/provision.bash".to_string()));
    }

    #[rstest]
    fn test_omakureignore_leading_slash_matches_root_relative(
        workspace_with_scripts: (TempDir, PathBuf),
    ) {
        let (_tmp, root) = workspace_with_scripts;
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/setup.py"), "print('nested')").unwrap();
        fs::write(root.join(".omakureignore"), "/setup.py\n").unwrap();
        let repo = FsWorkspaceRepository::new(&root);

        let scripts = repo.list_scripts_recursive().unwrap();
        let paths: Vec<String> = scripts
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(!paths.contains(&"setup.py".to_string()));
        assert!(paths.contains(&"nested/setup.py".to_string()));
    }

    #[rstest]
    fn test_absent_omakureignore_keeps_existing_behavior(
        workspace_with_scripts: (TempDir, PathBuf),
    ) {
        let (_tmp, root) = workspace_with_scripts;
        let repo = FsWorkspaceRepository::new(&root);

        let scripts = repo.list_scripts_recursive().unwrap();

        assert_eq!(scripts.len(), 4);
    }

    #[rstest]
    fn test_unreadable_omakureignore_degrades_gracefully(
        workspace_with_scripts: (TempDir, PathBuf),
    ) {
        let (_tmp, root) = workspace_with_scripts;
        fs::write(root.join(".omakureignore"), [0xff, 0xfe, 0xfd]).unwrap();
        let repo = FsWorkspaceRepository::new(&root);

        let scripts = repo.list_scripts_recursive().unwrap();

        assert_eq!(scripts.len(), 4);
    }

    #[rstest]
    fn test_list_entries_honors_omakureignore(workspace_with_scripts: (TempDir, PathBuf)) {
        let (_tmp, root) = workspace_with_scripts;
        fs::write(root.join(".omakureignore"), "infra/\nsetup.py\n").unwrap();
        let repo = FsWorkspaceRepository::new(&root);

        let entries = repo.list_entries(&root).unwrap();
        let names: Vec<String> = entries
            .iter()
            .map(|entry| {
                entry
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        assert!(!names.contains(&"infra".to_string()));
        assert!(!names.contains(&"setup.py".to_string()));
        assert!(names.contains(&"deploy.sh".to_string()));
    }

    #[test]
    fn test_list_entries_honors_nested_omakureignore_from_parent_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let scripts = root.join("scripts");
        fs::create_dir_all(scripts.join("helpers")).unwrap();
        fs::write(scripts.join("deploy.sh"), "#!/bin/bash").unwrap();
        fs::write(scripts.join("scratch.py"), "print('scratch')").unwrap();
        fs::write(scripts.join("helpers/internal.sh"), "#!/bin/bash").unwrap();
        fs::write(scripts.join(".omakureignore"), "helpers/\nscratch.py\n").unwrap();

        let repo = FsWorkspaceRepository::new(root);
        let entries = repo.list_entries(&scripts).unwrap();
        let names: Vec<String> = entries
            .iter()
            .map(|entry| {
                entry
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        assert!(names.contains(&"deploy.sh".to_string()));
        assert!(!names.contains(&"scratch.py".to_string()));
        assert!(!names.contains(&"helpers".to_string()));
    }

    #[test]
    fn test_recursive_scan_honors_nested_omakureignore_from_parent_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let scripts = root.join("scripts");
        fs::create_dir_all(scripts.join("helpers")).unwrap();
        fs::write(scripts.join("deploy.sh"), "#!/bin/bash").unwrap();
        fs::write(scripts.join("scratch.py"), "print('scratch')").unwrap();
        fs::write(scripts.join("helpers/internal.sh"), "#!/bin/bash").unwrap();
        fs::write(scripts.join(".omakureignore"), "helpers/\nscratch.py\n").unwrap();

        let repo = FsWorkspaceRepository::new(root);
        let found: Vec<String> = repo
            .list_scripts_recursive()
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(found.contains(&"scripts/deploy.sh".to_string()));
        assert!(!found.contains(&"scripts/scratch.py".to_string()));
        assert!(!found.contains(&"scripts/helpers/internal.sh".to_string()));
    }

    #[test]
    fn test_nested_omakureignore_combines_parent_and_child_rules() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let scripts = root.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(root.join("keep.sh"), "#!/bin/bash").unwrap();
        fs::write(scripts.join("visible.sh"), "#!/bin/bash").unwrap();
        fs::write(scripts.join("local.py"), "print('local')").unwrap();
        fs::write(scripts.join("global.tmp.sh"), "#!/bin/bash").unwrap();
        fs::write(root.join(".omakureignore"), "*.tmp.sh\n").unwrap();
        fs::write(scripts.join(".omakureignore"), "local.py\n").unwrap();

        let repo = FsWorkspaceRepository::new(root);
        let found: Vec<String> = repo
            .list_scripts_recursive()
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(found.contains(&"keep.sh".to_string()));
        assert!(found.contains(&"scripts/visible.sh".to_string()));
        assert!(!found.contains(&"scripts/local.py".to_string()));
        assert!(!found.contains(&"scripts/global.tmp.sh".to_string()));
    }

    #[test]
    fn test_nested_omakureignore_leading_slash_anchors_to_local_file_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let scripts = root.join("scripts");
        fs::create_dir_all(scripts.join("nested")).unwrap();
        fs::write(scripts.join("scratch.py"), "print('scratch')").unwrap();
        fs::write(scripts.join("nested/scratch.py"), "print('nested')").unwrap();
        fs::write(scripts.join(".omakureignore"), "/scratch.py\n").unwrap();

        let repo = FsWorkspaceRepository::new(root);
        let found: Vec<String> = repo
            .list_scripts_recursive()
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert!(!found.contains(&"scripts/scratch.py".to_string()));
        assert!(found.contains(&"scripts/nested/scratch.py".to_string()));
    }

    #[test]
    fn test_list_entries_outside_root_still_uses_root_ignore_only() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join(".omakureignore"), "hidden.sh\n").unwrap();
        fs::write(outside.join("hidden.sh"), "#!/bin/bash").unwrap();

        let repo = FsWorkspaceRepository::new(&root);
        let entries = repo.list_entries(&outside).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, outside.join("hidden.sh"));
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
    fn test_read_schema_unsupported_extension_returns_err() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("notes.txt");
        fs::write(&script, "no schema").unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        assert!(repo.read_schema(&script).is_err());
    }

    #[test]
    fn test_read_schema_powershell_uses_hash_or_semicolon_prefix() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.ps1");
        fs::write(
            &script,
            "; OMAKURE_SCHEMA_START\n; {\"Name\": \"Ps\", \"Fields\": []}\n; OMAKURE_SCHEMA_END\n",
        )
        .unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let schema = repo.read_schema(&script).unwrap();
        assert_eq!(schema.name, "Ps");
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
    fn test_should_skip_dir_omakure_metadata() {
        let tmp = TempDir::new().unwrap();
        let omakure = tmp.path().join(".omakure");
        fs::create_dir_all(&omakure).unwrap();
        assert!(should_skip_dir(&omakure));
    }

    #[test]
    fn test_should_not_skip_omaken_envs_as_special_case() {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join(".omaken").join("envs");
        fs::create_dir_all(&envs).unwrap();
        assert!(!should_skip_dir(&envs));
    }

    #[test]
    fn test_should_skip_dir_regular_envs_not_skipped() {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        assert!(!should_skip_dir(&envs));
    }
}
