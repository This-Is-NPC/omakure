mod environment;

use crate::domain::Schema;
use crate::error::AppResult;
use crate::ports::{ScriptRepository, ScriptRunOutput, ScriptRunner, WorkspaceEntry};
use std::io;
use std::path::Path;

pub struct ScriptService {
    repo: Box<dyn ScriptRepository>,
    // Both run paths now invoke `run_executor::execute_with_heartbeat`
    // directly. The runner is retained on the service so external
    // consumers (and the schema-discovery loop) can keep their existing
    // wiring without churn.
    #[allow(dead_code)]
    runner: Box<dyn ScriptRunner>,
}

pub use environment::EnvironmentService;

impl ScriptService {
    pub fn new(repo: Box<dyn ScriptRepository>, runner: Box<dyn ScriptRunner>) -> Self {
        Self { repo, runner }
    }

    pub fn list_entries(&self, dir: &Path) -> io::Result<Vec<WorkspaceEntry>> {
        self.repo.list_entries(dir)
    }

    pub fn load_schema(&self, script: &Path) -> AppResult<Schema> {
        self.repo.read_schema(script)
    }

    #[allow(dead_code)] // historical port; the new run_executor module is the active path
    pub fn run_script(&self, script: &Path, args: &[String]) -> AppResult<ScriptRunOutput> {
        self.runner.run(script, args)
    }
}
