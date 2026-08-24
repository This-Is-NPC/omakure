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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_repository_is_object_safe() {
        fn _assert_object_safe(_: &dyn ScriptRepository) {}
    }

    #[test]
    fn test_workspace_entry_kind_equality() {
        assert_eq!(WorkspaceEntryKind::Directory, WorkspaceEntryKind::Directory);
        assert_eq!(WorkspaceEntryKind::Script, WorkspaceEntryKind::Script);
        assert_ne!(WorkspaceEntryKind::Directory, WorkspaceEntryKind::Script);
    }
}
