mod environment;

use crate::domain::Schema;
use crate::error::AppResult;
use std::io;
use std::path::{Path, PathBuf};

pub use environment::{EnvFile, EnvPreview, EnvironmentConfig, EnvironmentRepository};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEntryKind {
    Directory,
    Script,
}

#[derive(Debug, Clone)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub kind: WorkspaceEntryKind,
}

pub trait ScriptRepository {
    fn list_entries(&self, dir: &Path) -> io::Result<Vec<WorkspaceEntry>>;
    fn list_scripts_recursive(&self) -> io::Result<Vec<PathBuf>>;
    fn read_schema(&self, script: &Path) -> AppResult<Schema>;
}

#[allow(dead_code)] // retained for future ScriptRunner implementations
#[derive(Debug, Clone)]
pub struct ScriptRunOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub success: bool,
}

#[allow(dead_code)] // both run paths now go through run_executor; trait kept as a port
pub trait ScriptRunner {
    fn run(&self, script: &Path, args: &[String]) -> AppResult<ScriptRunOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_repository_is_object_safe() {
        fn _assert_object_safe(_: &dyn ScriptRepository) {}
    }

    #[test]
    fn test_script_runner_is_object_safe() {
        fn _assert_object_safe(_: &dyn ScriptRunner) {}
    }

    #[test]
    fn test_workspace_entry_kind_equality() {
        assert_eq!(WorkspaceEntryKind::Directory, WorkspaceEntryKind::Directory);
        assert_eq!(WorkspaceEntryKind::Script, WorkspaceEntryKind::Script);
        assert_ne!(WorkspaceEntryKind::Directory, WorkspaceEntryKind::Script);
    }
}
