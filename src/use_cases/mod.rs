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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_script_service_list_entries() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("deploy.sh"), "#!/bin/bash").unwrap();
        fs::write(tmp.path().join("setup.py"), "print('hi')").unwrap();

        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let service = ScriptService::new(Box::new(repo), Box::new(runner));

        let entries = service.list_entries(tmp.path()).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_script_service_load_schema() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.sh");
        fs::write(
            &script,
            "#!/bin/bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"Test\", \"Fields\": []}\n# OMAKURE_SCHEMA_END\n",
        )
        .unwrap();

        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let service = ScriptService::new(Box::new(repo), Box::new(runner));

        let schema = service.load_schema(&script).unwrap();
        assert_eq!(schema.name, "Test");
    }

    #[test]
    #[cfg(unix)]
    fn test_script_service_run_script_via_runner() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("hi.sh");
        fs::write(&script, "#!/bin/bash\necho hi\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let service = ScriptService::new(Box::new(repo), Box::new(runner));

        let out = service.run_script(&script, &[]).unwrap();
        assert!(out.success);
        assert_eq!(out.stdout.trim(), "hi");
    }
}
