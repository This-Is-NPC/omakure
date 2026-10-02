use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_finalize_run_completed_updates_row() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let script = write_schema_script(&tmp, "ok.sh", r#"{"Name":"Ok","Fields":[]}"#, "true");
    let row = inline_row(&ws, &script);
    let result = ExecutionResult {
        terminal: ExecutionTerminal::Completed,
        completion: crate::runs::RunCompletion {
            stdout: "done\n".into(),
            stderr: String::new(),
            exit_code: Some(0),
            success: true,
            error: None,
        },
    };

    let final_row = finalize_run(&ws, &row.run_id, &result).unwrap();

    assert_eq!(final_row.state, RunState::Completed);
    assert_eq!(final_row.success, Some(true));
    assert_eq!(final_row.stdout, "done\n");
}

#[test]
fn test_finalize_run_failed_and_timed_out_update_row() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let fail_script =
        write_schema_script(&tmp, "fail.sh", r#"{"Name":"Fail","Fields":[]}"#, "false");
    let fail_row = runs::start_inline(
        &runs::open(&ws).unwrap(),
        fail_script.to_string_lossy().as_ref(),
        &[],
        "inline:fail",
        EnqueueOptions {
            run_id: Some("rid-fail".into()),
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let fail_result = ExecutionResult {
        terminal: ExecutionTerminal::Failed,
        completion: crate::runs::RunCompletion {
            stdout: String::new(),
            stderr: "boom\n".into(),
            exit_code: Some(1),
            success: false,
            error: None,
        },
    };
    let failed = finalize_run(&ws, &fail_row.run_id, &fail_result).unwrap();
    assert_eq!(failed.state, RunState::Failed);
    assert_eq!(failed.exit_code, Some(1));

    let timeout_script = write_schema_script(
        &tmp,
        "timeout.sh",
        r#"{"Name":"Timeout","Fields":[]}"#,
        "sleep 1",
    );
    let timeout_row = runs::start_inline(
        &runs::open(&ws).unwrap(),
        timeout_script.to_string_lossy().as_ref(),
        &[],
        "inline:timeout",
        EnqueueOptions {
            run_id: Some("rid-timeout".into()),
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let timeout_result = ExecutionResult {
        terminal: ExecutionTerminal::TimedOut,
        completion: crate::runs::RunCompletion {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(124),
            success: false,
            error: Some("timed out".into()),
        },
    };
    let timed_out = finalize_run(&ws, &timeout_row.run_id, &timeout_result).unwrap();
    assert_eq!(timed_out.state, RunState::TimedOut);
    assert_eq!(timed_out.error.as_deref(), Some("timed out"));
}

#[test]
fn test_finalize_run_cancelled_records_output() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let script = write_schema_script(
        &tmp,
        "cancel.sh",
        r#"{"Name":"Cancel","Fields":[]}"#,
        "sleep 1",
    );
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_string_lossy().as_ref(),
        &[],
        "inline:cancel",
        EnqueueOptions {
            run_id: Some("rid-cancel".into()),
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);
    runs::RunStore::open(&ws)
        .unwrap()
        .cancel(&row.run_id, Some("stop".into()))
        .unwrap();

    let result = ExecutionResult {
        terminal: ExecutionTerminal::Cancelled,
        completion: crate::runs::RunCompletion {
            stdout: "partial\n".into(),
            stderr: String::new(),
            exit_code: Some(130),
            success: false,
            error: Some("cancelled".into()),
        },
    };

    let final_row = finalize_run(&ws, &row.run_id, &result).unwrap();

    assert_eq!(final_row.state, RunState::Cancelled);
    assert_eq!(final_row.stdout, "partial\n");
    assert_eq!(final_row.exit_code, Some(130));
    assert!(
        final_row
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("cancelled")
    );
}

#[test]
#[cfg(unix)]
fn test_run_executes_script_and_persists_completed_row() {
    let tmp = TempDir::new().unwrap();
    let script = write_schema_script(&tmp, "ok.sh", r#"{"Name":"Ok","Fields":[]}"#, "true");

    run(
        tmp.path().to_path_buf(),
        RunArgs {
            script: script.to_string_lossy().to_string(),
            actor: "ai".into(),
            reason: Some("ship it".into()),
            run_id: Some("rid-run-ok".into()),
            parent_run_id: Some("parent-run".into()),
            no_prompt: false,
            env_file: None,
            secrets: vec![],
            args: vec![],
        },
        false,
    )
    .unwrap();

    let ws = workspace_in(&tmp);
    let conn = runs::open(&ws).unwrap();
    let row = runs::get_run(&conn, "rid-run-ok").unwrap().unwrap();
    assert_eq!(row.state, RunState::Completed);
    assert_eq!(row.actor, "ai");
    assert_eq!(row.reason.as_deref(), Some("ship it"));
    assert_eq!(row.parent_run_id.as_deref(), Some("parent-run"));
}

#[test]
#[cfg(unix)]
fn test_run_json_success_returns_ok_and_persists_row() {
    let tmp = TempDir::new().unwrap();
    let script = write_schema_script(&tmp, "json.sh", r#"{"Name":"Json","Fields":[]}"#, "true");

    run(
        tmp.path().to_path_buf(),
        RunArgs {
            script: script.to_string_lossy().to_string(),
            actor: "human".into(),
            reason: None,
            run_id: Some("rid-run-json".into()),
            parent_run_id: None,
            no_prompt: false,
            env_file: None,
            secrets: vec![],
            args: vec![],
        },
        true,
    )
    .unwrap();

    let ws = workspace_in(&tmp);
    let conn = runs::open(&ws).unwrap();
    let row = runs::get_run(&conn, "rid-run-json").unwrap().unwrap();
    assert_eq!(row.state, RunState::Completed);
}
