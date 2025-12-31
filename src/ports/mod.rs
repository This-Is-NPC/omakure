use crate::domain::Schema;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

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
    fn read_schema(&self, script: &Path) -> Result<Schema, Box<dyn Error>>;
}

#[derive(Debug, Clone)]
pub struct ScriptRunOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub success: bool,
}

pub trait ScriptRunner {
    fn run(&self, script: &Path, args: &[String]) -> Result<ScriptRunOutput, Box<dyn Error>>;
}
