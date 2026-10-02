use crate::operations::battery::{self, InspectBatteryRequest};
use crate::operations::core::{self, DescribeScriptRequest};
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::runs::{RunStore, WorkflowRun, WorkflowSnapshot, WorkflowStepSnapshot};
use crate::workspace::Workspace;

/// Start one explicitly selected workflow from scripts already installed in this workspace.
pub fn start_installed_workflow(
    workspace: &Workspace,
    battery_name: &str,
    workflow_id: &str,
) -> OperationResult<WorkflowRun> {
    let inspection = battery::inspect_battery(
        workspace,
        InspectBatteryRequest {
            name: battery_name.to_owned(),
        },
    )?;
    let workflow = inspection
        .manifest
        .workflows
        .iter()
        .find(|entry| entry.id == workflow_id)
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotFound,
                format!("battery workflow '{workflow_id}' was not found"),
            )
        })?;
    let commit = inspection.summary.resolved_commit.ok_or_else(|| {
        OperationError::new(OperationErrorCode::NotSynced, "battery has not been synced")
    })?;

    let mut steps = Vec::with_capacity(workflow.scripts.len());
    for script_id in &workflow.scripts {
        let script = inspection
            .manifest
            .scripts
            .iter()
            .find(|entry| &entry.id == script_id)
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::ManifestInvalid,
                    format!("workflow references unknown script '{script_id}'"),
                )
            })?;
        let provenance = battery::installed_script_provenance(workspace, battery_name, script_id)?
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::Conflict,
                    format!(
                        "battery script '{script_id}' must be installed before the workflow starts"
                    ),
                )
            })?;
        let expected_path = workspace.scripts_root().join(&script.path);
        if provenance.resolved_commit != commit
            || provenance.source_path != script.path
            || provenance.installed_path != expected_path
        {
            return Err(OperationError::new(
                OperationErrorCode::Conflict,
                format!(
                    "battery script '{script_id}' installation does not match the synced manifest"
                ),
            ));
        }
        let content_hash =
            battery::installed_script_matches_manifest(workspace, battery_name, script)?
                .ok_or_else(|| {
                    OperationError::new(
                        OperationErrorCode::Conflict,
                        format!("battery script '{script_id}' has changed since installation"),
                    )
                })?;
        let path =
            core::resolve_script_path(&expected_path.to_string_lossy(), workspace.scripts_root())?;
        let description = core::describe_script(
            workspace,
            DescribeScriptRequest {
                script: path.to_string_lossy().into_owned(),
            },
        )?;
        if let Some(field) = description
            .schema
            .fields
            .iter()
            .find(|field| field.required)
        {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                format!(
                    "workflow script '{script_id}' requires unsupported field '{}'",
                    field.name
                ),
            ));
        }
        if let Some(field) = description
            .schema
            .fields
            .iter()
            .find(|field| field.kind.eq_ignore_ascii_case("secret"))
        {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                format!(
                    "workflow script '{script_id}' declares unsupported secret field '{}'",
                    field.name
                ),
            ));
        }
        steps.push(WorkflowStepSnapshot {
            name: script_id.clone(),
            script_path: path.to_string_lossy().into_owned(),
            content_hash,
        });
    }

    let store = RunStore::open(workspace).map_err(run_error)?;
    store
        .start_workflow(
            WorkflowSnapshot {
                battery_id: battery_name.to_owned(),
                battery_version: inspection.manifest.battery.version,
                battery_commit: commit,
                workflow_name: workflow_id.to_owned(),
                steps,
            },
            "human",
        )
        .map_err(run_error)
}

pub fn workflow_status(
    workspace: &Workspace,
    workflow_run_id: &str,
) -> OperationResult<WorkflowRun> {
    if workflow_run_id.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "workflow run id is required",
        ));
    }
    let store = RunStore::open(workspace).map_err(run_error)?;
    store
        .get_workflow(workflow_run_id)
        .map_err(run_error)?
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotFound,
                format!("workflow run '{workflow_run_id}' was not found"),
            )
        })
}

fn run_error(error: crate::runs::RunsError) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::operations::battery::{
        AddBatteryRequest, InstallBatteryScriptRequest, SyncBatteryRequest,
    };
    use crate::runs::WorkflowState;
    use crate::test_support::{run_git, workspace_in};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn installed_battery_workflow_starts_and_rejects_missing_or_stale_installations() {
        let repo = TempDir::new().unwrap();
        let workspace_dir = TempDir::new().unwrap();
        let workspace = workspace_in(&workspace_dir);
        fs::create_dir_all(repo.path().join("scripts")).unwrap();
        fs::write(
            repo.path().join("omakure-battery.toml"),
            r#"
[battery]
name = "daily"
version = "1.0.0"

[[scripts]]
id = "daily.prepare"
path = "scripts/prepare.sh"

[[scripts]]
id = "daily.finish"
path = "scripts/finish.sh"

[[workflows]]
id = "daily.update"
scripts = ["daily.prepare", "daily.finish"]
"#,
        )
        .unwrap();
        for name in ["prepare", "finish"] {
            fs::write(
                repo.path().join(format!("scripts/{name}.sh")),
                format!(
                    "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {{\"Name\":\"{name}\",\"Fields\":[]}}\n# OMAKURE_SCHEMA_END\necho {name}\n"
                ),
            )
            .unwrap();
        }
        run_git(&["init", "-b", "main"], repo.path());
        run_git(&["add", "."], repo.path());
        run_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "initial",
            ],
            repo.path(),
        );
        battery::add_battery(
            &workspace,
            AddBatteryRequest {
                name: "daily".into(),
                git_url: repo.path().display().to_string(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();
        battery::sync_battery(
            &workspace,
            SyncBatteryRequest {
                name: "daily".into(),
            },
        )
        .unwrap();

        let missing = start_installed_workflow(&workspace, "daily", "daily.update").unwrap_err();
        assert_eq!(missing.code, OperationErrorCode::Conflict);
        assert!(missing.message.contains("must be installed"));

        for script_id in ["daily.prepare", "daily.finish"] {
            battery::install_battery_script(
                &workspace,
                InstallBatteryScriptRequest {
                    battery_name: "daily".into(),
                    script_id: script_id.into(),
                    force: false,
                },
            )
            .unwrap();
        }
        let started = start_installed_workflow(&workspace, "daily", "daily.update").unwrap();
        assert_eq!(started.state, WorkflowState::Running);
        assert_eq!(started.workflow_name, "daily.update");
        assert_eq!(started.steps.len(), 2);
        assert!(started.steps[0].run_id.is_some());
        assert!(started.steps[1].run_id.is_none());
        let status = workflow_status(&workspace, &started.workflow_id).unwrap();
        assert_eq!(status.workflow_id, started.workflow_id);
        assert_eq!(status.steps[0].run_id, started.steps[0].run_id);

        core::cancel_run(
            &workspace,
            core::CancelRunRequest {
                run_id: started.steps[0].run_id.clone().unwrap(),
                reason: Some("maintenance paused".into()),
            },
        )
        .unwrap();
        let cancelled = workflow_status(&workspace, &started.workflow_id).unwrap();
        assert_eq!(cancelled.state, WorkflowState::Cancelled);
        assert_eq!(
            cancelled.steps[0].error.as_deref(),
            Some("maintenance paused")
        );
        assert!(cancelled.steps[1].run_id.is_none());

        let installed_finish = workspace.scripts_root().join("scripts/finish.sh");
        let original = fs::read(&installed_finish).unwrap();
        let mut modified = original.clone();
        modified.extend_from_slice(b"# changed after installation\n");
        fs::write(&installed_finish, modified).unwrap();
        let changed = start_installed_workflow(&workspace, "daily", "daily.update").unwrap_err();
        assert_eq!(changed.code, OperationErrorCode::Conflict);
        assert!(changed.message.contains("has changed since installation"));
        fs::write(&installed_finish, original).unwrap();

        fs::write(repo.path().join("scripts/finish.sh"), "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\":\"finish\",\"Fields\":[]}\n# OMAKURE_SCHEMA_END\necho changed\n").unwrap();
        run_git(&["add", "."], repo.path());
        run_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "update",
            ],
            repo.path(),
        );
        battery::sync_battery(
            &workspace,
            SyncBatteryRequest {
                name: "daily".into(),
            },
        )
        .unwrap();
        let stale = start_installed_workflow(&workspace, "daily", "daily.update").unwrap_err();
        assert_eq!(stale.code, OperationErrorCode::Conflict);
        assert!(stale.message.contains("does not match the synced manifest"));
    }
}
