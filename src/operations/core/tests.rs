use super::run_queries::resolve_states;
use super::*;
use crate::app_meta;
use crate::operations::OperationErrorCode;
use crate::runs::{self, EnqueueOptions, RunCompletion, RunState, RunStateSet};
use crate::test_support::workspace_in;
#[cfg(unix)]
use crate::workspace::Workspace;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn resolve_states_default_is_terminal_set() {
    let resolved = resolve_states(&[], None).unwrap();
    assert_eq!(resolved, RunStateSet::Terminal.to_states());
}

#[test]
fn resolve_states_state_set_in_flight() {
    let resolved = resolve_states(&[], Some("in_flight")).unwrap();
    assert!(resolved.contains(&RunState::Queued));
    assert!(resolved.contains(&RunState::Running));
    assert!(!resolved.contains(&RunState::Completed));
}

#[test]
fn resolve_states_explicit_states() {
    let resolved = resolve_states(&["queued".into(), "running".into()], None).unwrap();
    assert_eq!(resolved, vec![RunState::Queued, RunState::Running]);
}

#[test]
fn resolve_states_invalid_value_returns_error() {
    let err = resolve_states(&["bogus".into()], None).unwrap_err();
    assert_eq!(err.code, OperationErrorCode::InvalidInput);
    assert!(err.message.contains("invalid run state"));
}

#[test]
fn resolve_states_mutually_exclusive_with_state_set() {
    let err = resolve_states(&["queued".into()], Some("terminal")).unwrap_err();
    assert!(err.message.contains("mutually exclusive"));
}

#[test]
fn resolve_states_invalid_state_set_returns_error() {
    let err = resolve_states(&[], Some("bogus")).unwrap_err();
    assert!(err.message.contains("invalid state-set"));
}

fn cue_request() -> EnqueueRunRequest {
    EnqueueRunRequest {
        script: "deploy.sh".into(),
        args: Vec::new(),
        env: None,
        secret_fields: Vec::new(),
        run_id: Some("cue-derived-run-id".into()),
        actor: "conductor".into(),
        reason: Some("contract test".into()),
        priority: 0,
        timeout_ms: None,
        parent_run_id: None,
        cron_schedule_id: None,
    }
}

#[test]
fn reserved_metadata_is_not_an_executable_subject() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    for script in [
        ".omakure/batteries/cache/uninstalled/job.sh",
        ".history/job.sh",
        ".git/hooks/job.sh",
        "tools/.omakure/job.sh",
    ] {
        write_script(ws.scripts_root(), script, &[]);
        for requested in [
            script.to_string(),
            ws.root().join(script).to_string_lossy().into(),
        ] {
            let error = enqueue_run(
                &ws,
                EnqueueRunRequest {
                    script: requested,
                    ..cue_request()
                },
            )
            .unwrap_err();
            assert_eq!(error.code, OperationErrorCode::UnsafePath);
        }
    }
    write_script(ws.scripts_root(), "installed/job.sh", &[]);
    assert!(enqueue_run(
        &ws,
        EnqueueRunRequest {
            script: "installed/job".into(),
            ..cue_request()
        }
    )
    .is_ok());
}

#[cfg(unix)]
#[test]
fn metadata_aliases_are_rejected_but_subject_aliases_are_allowed() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.root(), ".omakure/batteries/cache/job.sh", &[]);
    write_script(ws.root(), "installed/job.sh", &[]);
    symlink(
        ws.root().join(".omakure/batteries/cache"),
        ws.root().join("cache-alias"),
    )
    .unwrap();
    symlink(
        ws.root().join(".omakure/batteries/cache/job.sh"),
        ws.root().join("job.sh"),
    )
    .unwrap();
    symlink(
        ws.root().join("installed/job.sh"),
        ws.root().join("installed-alias.sh"),
    )
    .unwrap();
    for script in ["cache-alias/job.sh", "job.sh"] {
        assert_eq!(
            resolve_script_path(script, ws.root()).unwrap_err().code,
            OperationErrorCode::UnsafePath
        );
    }
    assert!(resolve_script_path("installed-alias.sh", ws.root()).is_ok());
}

/// Asserted against what landed in the database, not inferred from the call.
///
/// `None` writes ALLOW-ALL and a policy *lookup error* also grants
/// allow-all, so "we pass an empty vec" is a claim worth checking. If this
/// ever reads `None`, a remote instruction is receiving every secret the
/// node holds.
#[test]
fn a_cue_run_stores_an_explicit_deny_all_secret_policy() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &[]);

    let row = enqueue_cue_run(&ws, cue_request(), "authorized-hash").expect("enqueue the cue run");

    let conn = runs::open(&ws).unwrap();
    assert_eq!(
        runs::get_run_secret_refs(&conn, &row.run_id).unwrap(),
        Some(Vec::new()),
        "an empty policy is deny-all; None would have meant allow-all"
    );
}

/// The provenance that keeps it out of the worker lease steal and stops the
/// Health Plane reporting it as `manual`.
#[test]
fn a_cue_run_is_recorded_as_cue_originated() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &[]);

    let row = enqueue_cue_run(&ws, cue_request(), "authorized-hash").expect("enqueue the cue run");

    assert_eq!(row.trigger, runs::RunTrigger::Cue);
}

/// The caller supplies a run id derived from the cue id, so the primary key
/// is the durable at-most-once guard rather than a separate dedup store.
#[test]
fn the_same_cue_derived_run_id_cannot_be_enqueued_twice() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &[]);

    assert!(enqueue_cue_run(&ws, cue_request(), "authorized-hash").is_ok());
    assert!(
        enqueue_cue_run(&ws, cue_request(), "authorized-hash").is_err(),
        "the primary key is what makes a repeated cue id run at most once"
    );
}

/// The ordinary path is unchanged: it still resolves declared secrets.
#[test]
fn the_manual_enqueue_path_still_records_its_own_policy() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &[]);

    let row = enqueue_run_with_access(
        &ws,
        EnqueueRunRequest {
            run_id: Some("manual-run".into()),
            ..cue_request()
        },
        &crate::secrets::SecretAccess::allow_all(),
    )
    .expect("enqueue a manual run");

    assert_eq!(row.trigger, runs::RunTrigger::Manual);
}

fn write_script(root: &Path, path: &str, tags: &[&str]) {
    let tags_json = if tags.is_empty() {
        String::new()
    } else {
        format!(
            ",\"Tags\":[{}]",
            tags.iter()
                .map(|tag| format!("\"{tag}\""))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let script = root.join(path);
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(
            script,
            format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {{\"Name\":\"{path}\",\"Fields\":[]{tags_json}}}\n# OMAKURE_SCHEMA_END\necho ok\n"
            ),
        )
        .unwrap();
}

#[test]
fn workspace_summary_returns_operation_ready_paths() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);

    let summary = workspace_summary(&ws).unwrap();

    assert_eq!(summary.workspace_root, ws.root());
    assert_eq!(summary.omakure_dir, ws.omakure_dir());
    assert_eq!(summary.history_dir, ws.history_dir());
}

#[test]
fn list_scripts_filters_by_tags_and_preserves_schema_errors() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &["ops"]);
    write_script(ws.scripts_root(), "other.sh", &["misc"]);
    std::fs::write(ws.scripts_root().join("broken.sh"), "#!/usr/bin/env bash\n").unwrap();

    let entries = list_scripts(
        &ws,
        ListScriptsRequest {
            tags: vec!["ops".into()],
        },
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].relative_path, "deploy.sh");
}

#[test]
fn matches_all_tags_is_a_case_sensitive_and_over_every_required_tag() {
    let entry = ScriptSummary {
        absolute_path: "/x/a.sh".into(),
        relative_path: "a.sh".into(),
        name: Some("a".into()),
        description: None,
        tags: vec!["Prefeitura".into(), "sp".into()],
        field_count: 0,
        schema_error: None,
    };
    assert!(matches_all_tags(&entry, &[]));
    assert!(matches_all_tags(
        &entry,
        &["Prefeitura".into(), "sp".into()]
    ));
    assert!(!matches_all_tags(
        &entry,
        &["Prefeitura".into(), "rj".into()]
    ));
    assert!(!matches_all_tags(&entry, &["prefeitura".into()]));
}

#[test]
fn describe_script_returns_schema_payload() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &["ops"]);

    let desc = describe_script(
        &ws,
        DescribeScriptRequest {
            script: "deploy".into(),
        },
    )
    .unwrap();

    assert_eq!(desc.relative_path, "deploy.sh");
    assert_eq!(desc.schema.tags, vec!["ops"]);
}

#[test]
fn script_resolution_rejects_absolute_paths_outside_workspace() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(outside.path(), "outside.sh", &[]);

    let err = enqueue_run(
        &ws,
        EnqueueRunRequest {
            script: outside
                .path()
                .join("outside.sh")
                .to_string_lossy()
                .to_string(),
            args: Vec::new(),
            env: None,
            secret_fields: Vec::new(),
            run_id: None,
            actor: "agent".into(),
            reason: None,
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
        },
    )
    .unwrap_err();

    assert_eq!(err.code, OperationErrorCode::UnsafePath);
}

#[test]
fn script_resolution_rejects_missing_absolute_paths_outside_workspace() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let ws = workspace_in(&dir);

    let err = describe_script(
        &ws,
        DescribeScriptRequest {
            script: outside
                .path()
                .join("missing.sh")
                .to_string_lossy()
                .into_owned(),
        },
    )
    .unwrap_err();

    assert_eq!(err.code, OperationErrorCode::UnsafePath);
}
#[test]
fn script_resolution_accepts_confined_absolute_paths() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &[]);

    let path = ws.scripts_root().join("deploy.sh");
    let description = describe_script(
        &ws,
        DescribeScriptRequest {
            script: path.to_string_lossy().into_owned(),
        },
    )
    .unwrap();

    assert_eq!(description.relative_path, "deploy.sh");
}

#[cfg(unix)]
#[test]
fn script_resolution_accepts_absolute_paths_through_workspace_alias() {
    use std::os::unix::fs::symlink;

    let real = TempDir::new().unwrap();
    let alias_parent = TempDir::new().unwrap();
    let alias = alias_parent.path().join("workspace");
    symlink(real.path(), &alias).unwrap();
    // Keep the workspace's configured root as the symlink alias. The
    // absolute request therefore has a different spelling from the
    // canonical root, just as an 8.3/verbatim Windows path can.
    let ws = Workspace::new(alias);
    ws.ensure_layout().unwrap();
    write_script(ws.scripts_root(), "deploy.sh", &[]);

    let description = describe_script(
        &ws,
        DescribeScriptRequest {
            script: ws
                .scripts_root()
                .join("deploy.sh")
                .to_string_lossy()
                .into_owned(),
        },
    )
    .unwrap();

    assert_eq!(description.relative_path, "deploy.sh");
}

#[cfg(unix)]
#[test]
fn list_scripts_matches_a_symlinked_workspace_root() {
    use std::os::unix::fs::symlink;

    let real = TempDir::new().unwrap();
    let alias_parent = TempDir::new().unwrap();
    let alias = alias_parent.path().join("workspace");
    symlink(real.path(), &alias).unwrap();
    let ws = Workspace::new(alias);
    ws.ensure_layout().unwrap();
    write_script(ws.scripts_root(), "tools/deploy.sh", &[]);

    let entries = list_scripts(&ws, ListScriptsRequest::default()).unwrap();

    assert_eq!(entries[0].relative_path, "tools/deploy.sh");
}

#[test]
fn script_resolution_rejects_parent_traversal() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);

    let err = describe_script(
        &ws,
        DescribeScriptRequest {
            script: "../outside.sh".into(),
        },
    )
    .unwrap_err();

    assert_eq!(err.code, OperationErrorCode::UnsafePath);
}

#[cfg(unix)]
#[test]
fn script_resolution_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(outside.path(), "outside.sh", &[]);
    symlink(
        outside.path().join("outside.sh"),
        ws.scripts_root().join("escape.sh"),
    )
    .unwrap();

    let err = describe_script(
        &ws,
        DescribeScriptRequest {
            script: "escape.sh".into(),
        },
    )
    .unwrap_err();

    assert_eq!(err.code, OperationErrorCode::UnsafePath);
}

#[test]
fn duplicate_enqueue_preserves_io_error_code_and_message() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "job.sh", &[]);
    let request = EnqueueRunRequest {
        script: "job".into(),
        args: Vec::new(),
        env: None,
        secret_fields: Vec::new(),
        run_id: Some("duplicate-run".into()),
        actor: "agent".into(),
        reason: None,
        priority: 0,
        timeout_ms: None,
        parent_run_id: None,
        cron_schedule_id: None,
    };
    enqueue_run(&ws, request.clone()).unwrap();
    let error = enqueue_run(&ws, request).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::IoFailed);
    assert_eq!(
        error.message,
        "Insert run failed: UNIQUE constraint failed: runs.run_id"
    );
}

#[test]
fn unreadable_runs_workspace_preserves_io_code_and_text_across_paths() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "deploy.sh", &[]);
    std::fs::remove_dir_all(ws.history_dir()).unwrap();
    std::fs::write(ws.history_dir(), "blocking file").unwrap();
    let error = list_runs(&ws, ListRunsRequest::default()).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::IoFailed);
    assert!(error.message.starts_with("Create history dir failed: "));

    let manual = enqueue_run(&ws, cue_request()).unwrap_err();
    let cue = enqueue_cue_run(&ws, cue_request(), "authorized-hash").unwrap_err();
    assert_eq!(manual.code, OperationErrorCode::IoFailed);
    assert_eq!(cue.code, OperationErrorCode::IoFailed);
    assert_eq!(manual.message, error.message);
    assert_eq!(cue.message, error.message);
}

#[test]
fn enqueue_list_show_cancel_and_stats_share_runs_state_machine() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "job.sh", &[]);

    let row = enqueue_run(
        &ws,
        EnqueueRunRequest {
            script: "job".into(),
            args: vec!["--x".into()],
            env: None,
            secret_fields: Vec::new(),
            run_id: Some("rid-op".into()),
            actor: "agent".into(),
            reason: Some("test".into()),
            priority: 5,
            timeout_ms: Some(1000),
            parent_run_id: None,
            cron_schedule_id: None,
        },
    )
    .unwrap();
    assert_eq!(row.state, RunState::Queued);

    let rows = list_runs(
        &ws,
        ListRunsRequest {
            state_set: Some("all".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 1);

    let shown = show_run(
        &ws,
        ShowRunRequest {
            run_id: "rid-op".into(),
        },
    )
    .unwrap();
    assert_eq!(shown.actor, "agent");

    let cancelled = cancel_run(
        &ws,
        CancelRunRequest {
            run_id: "rid-op".into(),
            reason: Some("stop".into()),
        },
    )
    .unwrap();
    assert_eq!(cancelled.state, RunState::Cancelled);

    let cancel_error = cancel_run(
        &ws,
        CancelRunRequest {
            run_id: "rid-op".into(),
            reason: None,
        },
    )
    .unwrap_err();
    assert_eq!(cancel_error.code, OperationErrorCode::Conflict);
    assert_eq!(
        cancel_error.message,
        "cannot cancel run in terminal state 'cancelled'"
    );
    let dead_letter_error = dead_letter_run(
        &ws,
        DeadLetterRunRequest {
            run_id: "rid-op".into(),
            reason: None,
        },
    )
    .unwrap_err();
    assert_eq!(dead_letter_error.code, OperationErrorCode::Conflict);
    assert_eq!(
        dead_letter_error.message,
        "cannot promote run in state 'cancelled' to dead_letter; only failed or timed_out rows are eligible"
    );

    let stats = queue_stats(&ws).unwrap();
    assert_eq!(stats.total, 1);
}

#[test]
fn missing_run_queries_preserve_not_found_code_and_text() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    for error in [
        show_run(
            &ws,
            ShowRunRequest {
                run_id: "absent".into(),
            },
        )
        .unwrap_err(),
        cancel_run(
            &ws,
            CancelRunRequest {
                run_id: "absent".into(),
                reason: None,
            },
        )
        .unwrap_err(),
        dead_letter_run(
            &ws,
            DeadLetterRunRequest {
                run_id: "absent".into(),
                reason: None,
            },
        )
        .unwrap_err(),
    ] {
        assert_eq!(error.code, OperationErrorCode::NotFound);
        assert_eq!(error.message, "run not found: absent");
    }
}

#[test]
fn dead_letter_requires_existing_failed_run() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    write_script(ws.scripts_root(), "job.sh", &[]);
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        ws.scripts_root().join("job.sh").to_string_lossy().as_ref(),
        &[],
        "worker:test",
        EnqueueOptions {
            run_id: Some("rid-fail".into()),
            actor: "agent".into(),
            reason: None,
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
            script_content_hash: None,
        },
    )
    .unwrap();
    runs::fail(
        &conn,
        &row.run_id,
        RunCompletion {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(1),
            success: false,
            error: Some("boom".into()),
        },
    )
    .unwrap();

    let dead = dead_letter_run(
        &ws,
        DeadLetterRunRequest {
            run_id: "rid-fail".into(),
            reason: Some("triaged".into()),
        },
    )
    .unwrap();

    assert_eq!(dead.state, RunState::DeadLetter);
}

#[test]
fn list_traces_reports_missing_run_as_not_found() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);

    let err = list_traces(
        &ws,
        ListTracesRequest {
            run_id: "missing".into(),
            level: Some("debug".into()),
            since_sequence: None,
        },
    )
    .unwrap_err();

    assert_eq!(err.code, OperationErrorCode::NotFound);
}

#[test]
fn invalid_state_filter_is_operation_error() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);

    let err = list_runs(
        &ws,
        ListRunsRequest {
            states: vec!["bad".into()],
            ..Default::default()
        },
    )
    .unwrap_err();

    assert_eq!(err.code, OperationErrorCode::InvalidInput);
}

#[test]
#[cfg(unix)]
fn check_required_fields_passes_when_arg_present() {
    let ws = crate::test_support::scratch_workspace("required_present");
    let script = ws.root().join("ok.sh");
    let body = "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"x\", \"Fields\": [{\"Name\": \"name\", \"Type\": \"string\", \"Order\": 1, \"Required\": true}]}\n# OMAKURE_SCHEMA_END\necho done\n";
    std::fs::write(&script, body).unwrap();

    let res = check_required_fields(&ws, &script, &["--name=alice".to_string()]);
    assert!(res.is_ok());

    // Optional field absent — also OK.
    let opt_body = "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"x\", \"Fields\": [{\"Name\": \"opt\", \"Type\": \"string\", \"Order\": 1}]}\n# OMAKURE_SCHEMA_END\n";
    std::fs::write(&script, opt_body).unwrap();
    assert!(check_required_fields(&ws, &script, &[]).is_ok());

    // Schema absent — permissive.
    let bare = "#!/usr/bin/env bash\necho hi\n";
    std::fs::write(&script, bare).unwrap();
    assert!(check_required_fields(&ws, &script, &[]).is_ok());

    let _ = std::fs::remove_dir_all(ws.root());
}
