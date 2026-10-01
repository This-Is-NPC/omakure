use super::admission::parse_args_json;
use super::environment::bash_safe_current_exe;
use super::*;
use crate::adapters::environments::resolve_run_env;
use crate::runs::{self, EnqueueOptions, RunTrigger};
use crate::test_support::scratch_workspace;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
#[test]
fn queued_subject_replaced_with_metadata_symlink_is_not_executed() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = crate::test_support::workspace_in(&dir);
    let script = crate::test_support::write_bash_script(&ws, "job.sh", "echo installed");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "test",
        EnqueueOptions::default(),
    )
    .unwrap();
    fs::create_dir_all(ws.root().join(".omakure/batteries/cache")).unwrap();
    let cached = crate::test_support::write_bash_script(
        &ws,
        ".omakure/batteries/cache/job.sh",
        "echo unauthorized-execution",
    );
    fs::remove_file(&script).unwrap();
    std::os::unix::fs::symlink(cached, script).unwrap();

    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Errored);
    assert!(result
        .completion
        .error
        .unwrap()
        .contains("reserved workspace metadata"));
    assert!(result.completion.stdout.is_empty());
}

/// A Cue authorized one script; a baseline may legitimately replace it
/// before the worker claims the row. The bytes that run must be the bytes
/// that were authorized.
#[test]
#[cfg(unix)]
fn a_cue_run_refuses_a_script_that_changed_after_it_was_authorized() {
    let ws = scratch_workspace("cue_swapped_script");
    let script = crate::test_support::write_bash_script(&ws, "deploy.sh", "echo authorized");
    let authorized = crate::remote_cue::content_hash(&script).unwrap();
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "conductor".into(),
            omakure_version: "test".into(),
            trigger: RunTrigger::Cue,
            script_content_hash: Some(authorized),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    // The control: unchanged bytes still run, so the refusal below is
    // about the swap and not about Cue-origin runs being blocked outright.
    let unchanged = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(unchanged.terminal, ExecutionTerminal::Completed);
    assert!(unchanged.completion.stdout.contains("authorized"));

    crate::test_support::write_bash_script(&ws, "deploy.sh", "echo substituted");
    let swapped = execute_with_heartbeat(&ws, &row, vec![], None);

    assert_eq!(swapped.terminal, ExecutionTerminal::Failed);
    assert!(
        !swapped.completion.stdout.contains("substituted"),
        "the substituted script must not have run at all, got: {:?}",
        swapped.completion.stdout
    );
    assert!(swapped
        .completion
        .error
        .unwrap_or_default()
        .contains("changed after this remote run was authorized"));
    let _ = fs::remove_dir_all(ws.root());
}

/// The `run_secret_refs` lesson: "no record" must not read as "no
/// constraint". A Cue-origin row without a recorded hash is a row whose
/// authorization cannot be checked, and running it would make the whole
/// binding optional for anyone who could delete one table row.
#[test]
#[cfg(unix)]
fn a_cue_run_with_no_recorded_hash_does_not_execute() {
    let ws = scratch_workspace("cue_missing_hash");
    let script = crate::test_support::write_bash_script(&ws, "deploy.sh", "echo unconstrained");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "conductor".into(),
            omakure_version: "test".into(),
            trigger: RunTrigger::Cue,
            script_content_hash: None,
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let result = execute_with_heartbeat(&ws, &row, vec![], None);

    assert_eq!(result.terminal, ExecutionTerminal::Failed);
    assert!(
        !result.completion.stdout.contains("unconstrained"),
        "an unconstrained remote run must not reach the child process"
    );
    let _ = fs::remove_dir_all(ws.root());
}

/// The check is scoped to remote runs. A run someone started on this
/// machine has no earlier authorization to have drifted from, and
/// requiring a hash for it would break every local path.
#[test]
#[cfg(unix)]
fn a_manual_run_is_unaffected_by_the_authorized_content_check() {
    let ws = scratch_workspace("manual_unaffected");
    let script = crate::test_support::write_bash_script(&ws, "local.sh", "echo local");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: "test".into(),
            trigger: RunTrigger::Manual,
            script_content_hash: None,
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    crate::test_support::write_bash_script(&ws, "local.sh", "echo edited");
    let result = execute_with_heartbeat(&ws, &row, vec![], None);

    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert!(result.completion.stdout.contains("edited"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_completes_simple_script() {
    let ws = scratch_workspace("complete_simple");
    let script = crate::test_support::write_bash_script(&ws, "ok.sh", "echo hello");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert!(result.completion.stdout.contains("hello"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_injects_omakure_scripts_dir_env_var() {
    let ws = scratch_workspace("scripts_dir_env");
    let script =
        crate::test_support::write_bash_script(&ws, "echodir.sh", "echo $OMAKURE_SCRIPTS_DIR");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);
    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert!(
        result
            .completion
            .stdout
            .trim()
            .ends_with(ws.root().to_string_lossy().as_ref()),
        "expected stdout to end with workspace root, got: {:?}",
        result.completion.stdout
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_injects_omakure_bin_env_var() {
    let ws = scratch_workspace("omakure_bin_env");
    let script = crate::test_support::write_bash_script(&ws, "echobin.sh", "echo $OMAKURE_BIN");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);
    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    let expected = bash_safe_current_exe().expect("current_exe");
    assert!(!expected.is_empty());
    assert!(Path::new(&expected).is_absolute());
    assert_eq!(
        result.completion.stdout.trim(),
        expected,
        "expected OMAKURE_BIN to match bash-safe current_exe"
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_uses_redaction_file_instead_of_plaintext_secret_env() {
    let ws = scratch_workspace("redaction_file_env");
    let script = crate::test_support::write_bash_script(
        &ws,
        "redact-env.sh",
        r#"# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
if [ -n "$OMAKURE_REDACT_SECRETS" ]; then
  echo raw-redaction-env-present
  exit 2
fi
test -n "$OMAKURE_REDACT_SECRETS_FILE"
test -f "$OMAKURE_REDACT_SECRETS_FILE"
printf '%s\n' "$OMAKURE_REDACT_SECRETS_FILE"
"#,
    );
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let result = execute_with_heartbeat(
        &ws,
        &row,
        vec![("TOKEN".into(), "redaction-file-secret".into())],
        None,
    );

    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert!(!result.completion.stdout.contains("redaction-file-secret"));
    let redaction_file = result.completion.stdout.trim();
    assert!(!redaction_file.is_empty());
    assert!(!PathBuf::from(redaction_file).exists());
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_injects_omakure_run_id_env_var() {
    let ws = scratch_workspace("env_var");
    let script = crate::test_support::write_bash_script(&ws, "echoid.sh", "echo $OMAKURE_RUN_ID");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: "test".into(),
            run_id: Some("rid-fixed".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);
    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert!(
        result.completion.stdout.contains("rid-fixed"),
        "expected stdout to contain rid-fixed, got: {:?}",
        result.completion.stdout
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_reserved_vars_win_over_injected_extra_env() {
    // Precedence spec §1 layer 4: reserved vars are pushed AFTER
    // extra_env, so a user attempt to override OMAKURE_RUN_ID via the
    // injected env must lose (non-overridable).
    let ws = scratch_workspace("reserved_wins");
    let script = crate::test_support::write_bash_script(&ws, "echoid.sh", "echo $OMAKURE_RUN_ID");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            run_id: Some("rid-reserved".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let extra_env = vec![("OMAKURE_RUN_ID".to_string(), "HIJACKED".to_string())];
    let result = execute_with_heartbeat(&ws, &row, extra_env, None);

    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert!(
        result.completion.stdout.contains("rid-reserved"),
        "reserved run id should survive, got: {:?}",
        result.completion.stdout
    );
    assert!(
        !result.completion.stdout.contains("HIJACKED"),
        "injected value must not override reserved var"
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_reserved_scripts_dir_wins_over_injected_extra_env() {
    // Precedence spec §1 layer 4: OMAKURE_SCRIPTS_DIR is reserved just
    // like OMAKURE_RUN_ID and must be the final value observed by the
    // child, even if extra_env tries to hijack it.
    let ws = scratch_workspace("reserved_scripts_dir_wins");
    let script =
        crate::test_support::write_bash_script(&ws, "echodir.sh", "echo $OMAKURE_SCRIPTS_DIR");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let extra_env = vec![(
        "OMAKURE_SCRIPTS_DIR".to_string(),
        "/tmp/hijacked-omakure".to_string(),
    )];
    let result = execute_with_heartbeat(&ws, &row, extra_env, None);

    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert_eq!(result.completion.stdout.trim(), ws.root().to_string_lossy());
    assert!(!result.completion.stdout.contains("hijacked"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_reserved_vars_are_not_expandable_in_resolved_env() {
    // Reserved vars are applied by execute_with_heartbeat after
    // resolve_run_env has expanded user layers, so references to them in
    // active env files expand as undefined. The reserved var itself is
    // still injected afterward and visible to the child.
    let ws = scratch_workspace("reserved_not_expandable");
    let envs = ws.envs_dir();
    fs::create_dir_all(envs).unwrap();
    fs::write(envs.join("dev.conf"), "PLAIN=$OMAKURE_RUN_ID\n").unwrap();
    fs::write(envs.join("active"), "dev.conf\n").unwrap();
    let script = crate::test_support::write_bash_script(
        &ws,
        "echoenv.sh",
        "echo \"${PLAIN}|${OMAKURE_RUN_ID}\"",
    );
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            run_id: Some("rid-real-pipeline".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let extra_env = resolve_run_env(envs, None).unwrap();
    let result = execute_with_heartbeat(&ws, &row, extra_env, None);

    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert_eq!(result.completion.stdout.trim(), "|rid-real-pipeline");
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_failed_script_marked_failed() {
    let ws = scratch_workspace("failed");
    let script = crate::test_support::write_bash_script(&ws, "bad.sh", "exit 7");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);
    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Failed);
    assert_eq!(result.completion.exit_code, Some(7));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_timeout_kills_long_script() {
    let ws = scratch_workspace("timeout");
    let script = crate::test_support::write_bash_script(&ws, "sleep.sh", "sleep 5");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            timeout_ms: Some(500),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);
    let started = Instant::now();
    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(3));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn execute_returns_errored_when_script_missing() {
    let ws = scratch_workspace("missing_script");
    let conn = runs::open(&ws).unwrap();
    // Enqueue a row pointing at a path that does not exist.
    let bogus = ws.root().join("does_not_exist.sh");
    let row = runs::start_inline(
        &conn,
        bogus.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Errored);
    assert!(result
        .completion
        .error
        .as_deref()
        .unwrap_or("")
        .contains("script not found"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_returns_errored_for_unsupported_extension() {
    let ws = scratch_workspace("unsupported_ext");
    let script = ws.root().join("plain.txt");
    fs::write(&script, "not a script").unwrap();
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Errored);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_fails_when_required_field_missing() {
    let ws = scratch_workspace("missing_required");
    let script = crate::test_support::write_bash_script(
        &ws,
        "needs.sh",
        r#"# placeholder
echo done"#,
    );
    // Inject a schema block at the top of the script declaring a
    // required `--name` field.
    let body = "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"x\", \"Fields\": [{\"Name\": \"name\", \"Type\": \"string\", \"Order\": 1, \"Required\": true}]}\n# OMAKURE_SCHEMA_END\necho done\n";
    fs::write(&script, body).unwrap();
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    assert_eq!(result.terminal, ExecutionTerminal::Failed);
    assert!(result
        .completion
        .error
        .as_deref()
        .unwrap_or("")
        .contains("required field"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
#[cfg(unix)]
fn execute_resolves_persisted_secret_ref_at_runtime_and_redacts_output() {
    let ws = scratch_workspace("secret_ref_runtime");
    fs::write(
        ws.envs_dir().join("prod.conf"),
        "TOKEN=from_file_provider\n",
    )
    .unwrap();
    let script = crate::test_support::write_bash_script(&ws, "secret_ref.sh", "");
    let body = r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"SecretRef","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
if [ "$1" = "--token=from_file_provider" ]; then echo "matched from_file_provider"; else echo "leaked:$1"; exit 7; fi
"#;
    fs::write(&script, body).unwrap();
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &["--token=secret://prod/token".to_string()],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let result = execute_with_heartbeat(&ws, &row, vec![], None);

    assert_eq!(result.terminal, ExecutionTerminal::Completed);
    assert!(result.completion.stdout.contains("matched <redacted>"));
    assert!(!result.completion.stdout.contains("from_file_provider"));
    assert!(!result.completion.stderr.contains("from_file_provider"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn parse_args_json_handles_invalid_input() {
    assert!(parse_args_json("not valid json").is_empty());
    assert_eq!(parse_args_json("[\"a\",\"b\"]"), vec!["a", "b"]);
}

#[test]
#[cfg(unix)]
fn execute_external_cancel_kills_running_script() {
    let ws = scratch_workspace("cancel");
    let script = crate::test_support::write_bash_script(&ws, "sleep.sh", "sleep 10");
    let conn = runs::open(&ws).unwrap();
    let row = runs::start_inline(
        &conn,
        script.to_str().unwrap(),
        &[],
        "inline:test",
        EnqueueOptions {
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    // Spawn a thread that flips the row to cancelled after a delay.
    let ws_thread = ws.clone_for_executor();
    let id = row.run_id.clone();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        let conn = runs::open(&ws_thread).unwrap();
        runs::cancel(&conn, &id, Some("user".into()), None).unwrap();
    });

    let started = Instant::now();
    let result = execute_with_heartbeat(&ws, &row, vec![], None);
    canceller.join().unwrap();
    assert_eq!(result.terminal, ExecutionTerminal::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(8));
    let _ = fs::remove_dir_all(ws.root());
}
