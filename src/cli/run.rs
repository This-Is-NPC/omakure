//! `omakure run` — synchronous fast path for one script execution.
//!
//! `omakure run` writes through the same state machine as
//! `omakure queue worker`. The row is inserted in `state='running'` at
//! start (so `history list --state running` sees it immediately) and
//! transitions to `completed`/`failed`/`timed_out` on completion via
//! the shared [`crate::run_executor::execute_with_heartbeat`] helper.

use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::app_meta;
use crate::cli::args::RunArgs;
use crate::cli::json::{self, codes};
use crate::ports::ScriptRepository;
use crate::run_executor::{execute_with_heartbeat, ExecutionTerminal};
use crate::runs::{self, EnqueueOptions};
use crate::runtime::script_extensions;
use crate::workspace::Workspace;
use std::error::Error;
use std::path::{Path, PathBuf};

pub fn run(
    scripts_dir: PathBuf,
    options: RunArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let script_path = match resolve_script_path(&options.script, workspace.root()) {
        Ok(path) => path,
        Err(err) => return emit_error(json_output, codes::NOT_FOUND, err.to_string()),
    };

    // Layers 2 + 3 of the env-injection precedence table
    // (`.docs/env-injection-spec.md` §1): the active managed env, with the
    // optional CLI `--env-file` folded on top (env-file wins per key). The
    // reserved vars `OMAKURE_RUN_ID` / `OMAKURE_SCRIPTS_DIR` (layer 4) are
    // pushed after this inside `execute_with_heartbeat`, stay
    // non-overridable, and are therefore not visible to `$VAR` expansion in
    // `.conf` / `--env-file` values. A missing/unreadable `--env-file` is a
    // hard error.
    let extra_env = match crate::adapters::environments::resolve_run_env(
        workspace.envs_dir(),
        options.env_file.as_deref(),
    ) {
        Ok(env) => env,
        Err(err) => return emit_error(json_output, codes::INVALID_ARGUMENT, err.to_string()),
    };

    let direct_secrets = match crate::secrets::parse_direct_secrets(&options.secrets) {
        Ok(secrets) => secrets,
        Err(err) => return emit_error(json_output, codes::INVALID_ARGUMENT, err),
    };

    let resolved_args = match crate::secrets::resolve_args_with_direct_secrets(
        &workspace,
        &script_path,
        &options.args,
        &extra_env,
        &direct_secrets,
    ) {
        Ok(resolved) => resolved,
        Err((field, message)) => {
            return emit_error(
                json_output,
                codes::MISSING_REQUIRED_FIELD,
                format!("required field `{}` is missing: {}", field, message),
            );
        }
    };

    // `--json` implies `--no-prompt`: agents must never block on a TTY.
    let no_prompt = options.no_prompt || json_output;
    if no_prompt {
        if let Err((field, message)) =
            check_required_fields(&workspace, &script_path, &resolved_args.persisted_args)
        {
            return emit_error(
                json_output,
                codes::MISSING_REQUIRED_FIELD,
                format!("required field `{}` is missing: {}", field, message),
            );
        }
    }

    let canonical = std::fs::canonicalize(&script_path).unwrap_or_else(|_| script_path.clone());
    let canonical_str = canonical.to_string_lossy().to_string();
    let conn = runs::open(&workspace).map_err(|err| -> Box<dyn Error> { err.into() })?;
    let row = runs::start_inline(
        &conn,
        &canonical_str,
        &resolved_args.persisted_args,
        &format!("inline:{}", std::process::id()),
        EnqueueOptions {
            run_id: options.run_id.clone(),
            actor: options.actor.clone(),
            reason: options.reason.clone(),
            priority: 0,
            timeout_ms: None,
            parent_run_id: options.parent_run_id.clone(),
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: crate::runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
        },
    )
    .map_err(|err| -> Box<dyn Error> { err.into() })?;
    drop(conn);
    let mut execution_row = row.clone();
    execution_row.args_json = serde_json::to_string(&resolved_args.execution_args)
        .unwrap_or_else(|_| row.args_json.clone());
    let result = execute_with_heartbeat(&workspace, &execution_row, extra_env, None);

    let final_row = finalize_run(&workspace, &row.run_id, &result);

    if json_output {
        if let Some(row) = final_row {
            json::print_ok(row);
        }
        // Match the previous PR #8 surface: a failing script propagates
        // its exit code so the caller's CI/agent loop sees the right
        // signal even under --json.
        if matches!(
            result.terminal,
            ExecutionTerminal::Failed
                | ExecutionTerminal::TimedOut
                | ExecutionTerminal::Errored
                | ExecutionTerminal::Cancelled
        ) {
            std::process::exit(result.completion.exit_code.unwrap_or(1));
        }
        return Ok(());
    }

    if !result.completion.stdout.trim().is_empty() {
        print!("{}", result.completion.stdout);
        if !result.completion.stdout.ends_with('\n') {
            println!();
        }
    }
    if !result.completion.stderr.trim().is_empty() {
        eprint!("{}", result.completion.stderr);
        if !result.completion.stderr.ends_with('\n') {
            eprintln!();
        }
    }
    if let Some(err) = &result.completion.error {
        eprintln!("error: {}", err);
    }
    if matches!(
        result.terminal,
        ExecutionTerminal::Failed
            | ExecutionTerminal::TimedOut
            | ExecutionTerminal::Errored
            | ExecutionTerminal::Cancelled
    ) {
        std::process::exit(result.completion.exit_code.unwrap_or(1));
    }
    Ok(())
}

fn finalize_run(
    workspace: &Workspace,
    run_id: &str,
    result: &crate::run_executor::ExecutionResult,
) -> Option<runs::RunRow> {
    let conn = runs::open(workspace).ok()?;
    let _ = match result.terminal {
        ExecutionTerminal::Completed => runs::complete(&conn, run_id, result.completion.clone()),
        ExecutionTerminal::Failed | ExecutionTerminal::Errored => {
            runs::fail(&conn, run_id, result.completion.clone())
        }
        ExecutionTerminal::TimedOut => runs::time_out(&conn, run_id, result.completion.clone()),
        ExecutionTerminal::Cancelled => {
            // The cancel transition was already recorded by an external
            // caller; just attach the captured output.
            runs::record_cancelled_output(&conn, run_id, result.completion.clone())
        }
    };
    runs::get_run(&conn, run_id).ok().flatten()
}

/// Verify that every required field on the script's schema either has a
/// `--<field>` (or its `Arg` override) on the command line, or is
/// non-required. Returns `Err((field_name, message))` if any required
/// field is missing.
fn check_required_fields(
    workspace: &Workspace,
    script_path: &Path,
    args: &[String],
) -> Result<(), (String, String)> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let schema = match repo.read_schema(script_path) {
        Ok(s) => s,
        // No schema means no required-field check is possible. Treat as
        // a permissive pass — the script may have its own validation.
        Err(_) => return Ok(()),
    };
    for field in &schema.fields {
        if !field.required.unwrap_or(false) {
            continue;
        }
        let arg_flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        if !cli_args_contain_flag(args, &arg_flag) {
            return Err((
                field.name.clone(),
                format!("expected `{}` on the command line", arg_flag),
            ));
        }
    }
    Ok(())
}

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

pub(crate) fn resolve_script_path(
    script: &str,
    scripts_dir: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    let has_separator = script.contains('/') || script.contains('\\');
    let path = PathBuf::from(script);

    if path.is_absolute() {
        return resolve_with_extensions(path);
    }

    if has_separator {
        return resolve_with_extensions(scripts_dir.join(path));
    }

    resolve_with_extensions(scripts_dir.join(script))
}

pub(crate) fn cli_args_contain_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{}=", flag)))
}

fn resolve_with_extensions(path: PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    if path.exists() {
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("Script is not a file: {}", path.display()).into());
    }
    if path.extension().is_some() {
        return Err(format!("Script not found: {}", path.display()).into());
    }
    for ext in script_extensions() {
        let mut candidate = path.clone();
        candidate.set_extension(ext);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("Script not found: {}", path.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_executor::{ExecutionResult, ExecutionTerminal};
    use crate::runs::RunState;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn write_schema_script(tmp: &TempDir, name: &str, schema_json: &str, body: &str) -> PathBuf {
        let path = tmp.path().join(name);
        write_file(
            &path,
            &format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {}\n# OMAKURE_SCHEMA_END\n{}\n",
                schema_json, body
            ),
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn make_workspace(tmp: &TempDir) -> Workspace {
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        ws
    }

    fn inline_row(workspace: &Workspace, script: &Path) -> crate::runs::RunRow {
        let conn = runs::open(workspace).unwrap();
        runs::start_inline(
            &conn,
            script.to_string_lossy().as_ref(),
            &[],
            "inline:test",
            EnqueueOptions {
                run_id: Some("rid-inline".into()),
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[rstest]
    #[case::exact_match(&["--target", "prod"], "--target", true)]
    #[case::equals_syntax(&["--target=prod"], "--target", true)]
    #[case::not_present(&["--other", "val"], "--target", false)]
    #[case::empty_args(&[], "--target", false)]
    fn test_cli_args_contain_flag(
        #[case] args: &[&str],
        #[case] flag: &str,
        #[case] expected: bool,
    ) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        assert_eq!(cli_args_contain_flag(&args, flag), expected);
    }

    #[test]
    fn test_resolve_script_path_exact_file() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("deploy.sh");
        fs::write(&script, "#!/bin/bash").unwrap();

        let result = resolve_script_path("deploy.sh", tmp.path()).unwrap();
        assert_eq!(result, script);
    }

    #[test]
    fn test_resolve_script_path_extension_fallback() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("deploy.sh");
        fs::write(&script, "#!/bin/bash").unwrap();

        let result = resolve_script_path("deploy", tmp.path()).unwrap();
        assert_eq!(result, script);
    }

    #[test]
    fn test_resolve_script_path_not_found() {
        let tmp = TempDir::new().unwrap();
        let result = resolve_script_path("nonexistent", tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_script_path_absolute() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("abs.sh");
        fs::write(&script, "#!/bin/bash").unwrap();

        let result = resolve_script_path(&script.to_string_lossy(), tmp.path()).unwrap();
        assert_eq!(result, script);
    }

    #[test]
    fn test_resolve_script_path_with_separator() {
        let tmp = TempDir::new().unwrap();
        let subdir = tmp.path().join("infra");
        fs::create_dir_all(&subdir).unwrap();
        let script = subdir.join("deploy.sh");
        fs::write(&script, "#!/bin/bash").unwrap();

        let result = resolve_script_path("infra/deploy.sh", tmp.path()).unwrap();
        assert_eq!(result, script);
    }

    #[test]
    fn test_resolve_script_path_directory_not_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("deploy.sh");
        fs::create_dir_all(&dir).unwrap();

        let result = resolve_script_path("deploy.sh", tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a file"));
    }

    #[test]
    fn test_check_required_fields_without_schema_is_permissive() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let script = tmp.path().join("plain.sh");
        write_file(&script, "#!/usr/bin/env bash\necho hi\n");

        assert!(check_required_fields(&ws, &script, &[]).is_ok());
    }

    #[test]
    fn test_check_required_fields_accepts_default_and_override_flags() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let script = write_schema_script(
            &tmp,
            "deploy.sh",
            r#"{"Name":"Deploy","Fields":[{"Name":"target","Type":"string","Order":1,"Required":true},{"Name":"region","Type":"string","Order":2,"Required":true,"Arg":"--azure-region"},{"Name":"optional","Type":"string","Order":3,"Required":false}]}"#,
            "echo hi",
        );

        let args = vec![
            "--target=prod".to_string(),
            "--azure-region".to_string(),
            "eastus".to_string(),
        ];

        assert!(check_required_fields(&ws, &script, &args).is_ok());
    }

    #[test]
    fn test_check_required_fields_returns_missing_field_and_message() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let script = write_schema_script(
            &tmp,
            "deploy.sh",
            r#"{"Name":"Deploy","Fields":[{"Name":"target","Type":"string","Order":1,"Required":true}]}"#,
            "echo hi",
        );

        let err = check_required_fields(&ws, &script, &[]).unwrap_err();

        assert_eq!(err.0, "target");
        assert!(err.1.contains("--target"));
    }

    #[test]
    fn test_finalize_run_completed_updates_row() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
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
        let ws = make_workspace(&tmp);
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
        let ws = make_workspace(&tmp);
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
        runs::cancel(&conn, &row.run_id, Some("stop".into()), None).unwrap();
        drop(conn);

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
        assert!(final_row
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("cancelled"));
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

        let ws = make_workspace(&tmp);
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

        let ws = make_workspace(&tmp);
        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-run-json").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed);
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn read_all_bytes_under(dir: &Path) -> Vec<u8> {
        let mut buf = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(bytes) = fs::read(&path) {
                        buf.extend_from_slice(&bytes);
                    }
                } else if path.is_dir() {
                    buf.extend_from_slice(&read_all_bytes_under(&path));
                }
            }
        }
        buf
    }

    // CALL SITE: `omakure run` (cli/run.rs). The active managed env must
    // reach the spawned process; the script echoes an injected var and the
    // value must land in the persisted run record's stdout.
    #[test]
    #[cfg(unix)]
    fn test_run_injects_active_env_into_script_and_persists_output() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let envs = ws.envs_dir();
        fs::write(envs.join("dev.conf"), "INJECTED_VAR=cli_injected_42").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let script = write_schema_script(
            &tmp,
            "echo.sh",
            r#"{"Name":"Echo","Fields":[]}"#,
            "echo \"$INJECTED_VAR\"",
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-inject-cli".into()),
                parent_run_id: None,
                no_prompt: false,
                env_file: None,
                secrets: vec![],
                args: vec![],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-inject-cli").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed);
        assert!(
            row.stdout.contains("cli_injected_42"),
            "expected injected var in stdout, got: {:?}",
            row.stdout
        );
    }

    // REDACTION (secret-non-persistence gate, spec §3): an injected
    // secret-looking var must NEVER be written to runs.sqlite / its WAL /
    // logs / the trace. The env's sole consumer is `cmd.env` in
    // `MultiScriptRunner::build_command`; the persistence writers
    // (`runs::insert_run`, `run_traces`) never receive it. The script does
    // NOT echo the secret (echoing would legitimately place it in stdout,
    // which is persisted).
    #[test]
    #[cfg(unix)]
    fn test_injected_secret_not_persisted_to_storage() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let envs = ws.envs_dir();
        fs::write(
            envs.join("dev.conf"),
            "MY_SECRET_TOKEN=supersecret_do_not_persist",
        )
        .unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let script = write_schema_script(
            &tmp,
            "quiet.sh",
            r#"{"Name":"Quiet","Fields":[]}"#,
            "echo ok",
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-redact".into()),
                parent_run_id: None,
                no_prompt: false,
                env_file: None,
                secrets: vec![],
                args: vec![],
            },
            false,
        )
        .unwrap();

        // Sanity: the run really executed and persisted.
        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-redact").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed);
        assert!(row.stdout.contains("ok"));
        drop(conn);

        // Scan every persisted file (runs.sqlite + WAL/shm + search index)
        // for the secret value. It must be absent everywhere.
        let bytes = read_all_bytes_under(ws.history_dir());
        assert!(
            !contains_subslice(&bytes, b"supersecret_do_not_persist"),
            "injected secret value leaked into persistent storage"
        );
    }

    // CALL SITE: `omakure run --env-file` (layer 3, spec §1). A var defined
    // only in the passed env-file must reach the spawned process and land in
    // the persisted stdout.
    #[test]
    #[cfg(unix)]
    fn test_run_env_file_var_reaches_script() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let env_file = tmp.path().join("run.env");
        fs::write(&env_file, "FROM_FILE=file_value_99").unwrap();

        let script = write_schema_script(
            &tmp,
            "echo.sh",
            r#"{"Name":"Echo","Fields":[]}"#,
            "echo \"$FROM_FILE\"",
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-envfile".into()),
                parent_run_id: None,
                no_prompt: false,
                env_file: Some(env_file),
                secrets: vec![],
                args: vec![],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-envfile").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed);
        assert!(
            row.stdout.contains("file_value_99"),
            "expected env-file var in stdout, got: {:?}",
            row.stdout
        );
    }

    // PRECEDENCE (spec §1): a key set in BOTH the managed active env AND the
    // --env-file resolves to the --env-file value in the spawned process.
    #[test]
    #[cfg(unix)]
    fn test_run_env_file_overrides_active_env() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let envs = ws.envs_dir();
        fs::write(envs.join("dev.conf"), "SHARED=from_active").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let env_file = tmp.path().join("run.env");
        fs::write(&env_file, "SHARED=from_file").unwrap();

        let script = write_schema_script(
            &tmp,
            "echo.sh",
            r#"{"Name":"Echo","Fields":[]}"#,
            "echo \"$SHARED\"",
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-precedence".into()),
                parent_run_id: None,
                no_prompt: false,
                env_file: Some(env_file),
                secrets: vec![],
                args: vec![],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-precedence").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed);
        assert!(
            row.stdout.contains("from_file"),
            "env-file must override active env, got: {:?}",
            row.stdout
        );
        assert!(
            !row.stdout.contains("from_active"),
            "active-env value must be shadowed by the env-file, got: {:?}",
            row.stdout
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_run_resolves_secret_from_env_file_arg_and_redacts_persistence() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let envs = ws.envs_dir();
        fs::write(envs.join("dev.conf"), "TOKEN=from_active").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();
        let env_file = tmp.path().join("selected.env");
        fs::write(&env_file, "TOKEN=from_selected_secret").unwrap();

        let script = write_schema_script(
            &tmp,
            "secret_arg.sh",
            r#"{"Name":"SecretArg","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            r#"if [ "$2" = "from_selected_secret" ]; then echo matched; else echo "leaked:$2"; exit 7; fi"#,
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-secret-env".into()),
                parent_run_id: None,
                no_prompt: true,
                env_file: Some(env_file),
                secrets: vec![],
                args: vec![],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-secret-env").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed, "stderr: {}", row.stderr);
        assert!(row.stdout.contains("matched"));
        assert!(!row.args_json.contains("from_selected_secret"));
        assert!(!row.stdout.contains("from_selected_secret"));
        assert!(!row.stderr.contains("from_selected_secret"));
    }

    #[test]
    #[cfg(unix)]
    fn test_run_direct_secret_arg_wins_and_is_redacted() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        let envs = ws.envs_dir();
        fs::write(envs.join("dev.conf"), "TOKEN=from_active").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let script = write_schema_script(
            &tmp,
            "direct_secret.sh",
            r#"{"Name":"DirectSecret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            r#"if [ "$2" = "direct_secret_value" ]; then echo matched; else echo "leaked:$2"; exit 7; fi"#,
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-secret-direct".into()),
                parent_run_id: None,
                no_prompt: true,
                env_file: None,
                secrets: vec![],
                args: vec!["--token".into(), "direct_secret_value".into()],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-secret-direct").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed, "stderr: {}", row.stderr);
        assert!(row.stdout.contains("matched"));
        assert!(row.args_json.contains("<redacted>"));
        assert!(!row.args_json.contains("direct_secret_value"));
    }

    #[test]
    #[cfg(unix)]
    fn test_run_secret_option_supplies_direct_secret_and_redacts() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);

        let script = write_schema_script(
            &tmp,
            "secret_option.sh",
            r#"{"Name":"SecretOption","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            r#"if [ "$2" = "from_secret_option" ]; then echo matched; else echo "leaked:$2"; exit 7; fi"#,
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-secret-option".into()),
                parent_run_id: None,
                no_prompt: true,
                env_file: None,
                secrets: vec!["TOKEN=from_secret_option".into()],
                args: vec![],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-secret-option").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed, "stderr: {}", row.stderr);
        assert!(row.stdout.contains("matched"));
        assert!(row.args_json.contains("<redacted>"));
        assert!(!row.args_json.contains("from_secret_option"));
    }

    #[test]
    #[cfg(unix)]
    fn test_run_secret_ref_arg_resolves_file_provider_and_redacts() {
        let tmp = TempDir::new().unwrap();
        let ws = make_workspace(&tmp);
        fs::write(
            ws.envs_dir().join("prod.conf"),
            "TOKEN=from_file_provider\n",
        )
        .unwrap();

        let script = write_schema_script(
            &tmp,
            "secret_ref.sh",
            r#"{"Name":"SecretRef","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            r#"if [ "$1" = "--token=from_file_provider" ]; then echo "matched from_file_provider"; else echo "leaked:$1"; exit 7; fi"#,
        );

        run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-secret-ref".into()),
                parent_run_id: None,
                no_prompt: true,
                env_file: None,
                secrets: vec![],
                args: vec!["--token=secret://prod/token".into()],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-secret-ref").unwrap().unwrap();
        assert_eq!(row.state, RunState::Completed, "stderr: {}", row.stderr);
        assert!(row.stdout.contains("matched <redacted>"));
        assert!(row.args_json.contains("--token=secret://prod/token"));
        assert!(!row.args_json.contains("from_file_provider"));
        assert!(!row.stdout.contains("from_file_provider"));
        assert!(!row.stderr.contains("from_file_provider"));
    }

    #[test]
    #[cfg(unix)]
    fn test_run_missing_required_secret_is_error() {
        let tmp = TempDir::new().unwrap();
        let script = write_schema_script(
            &tmp,
            "missing_secret.sh",
            r#"{"Name":"MissingSecret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            "true",
        );

        let result = run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-missing-secret".into()),
                parent_run_id: None,
                no_prompt: true,
                env_file: None,
                secrets: vec![],
                args: vec![],
            },
            false,
        );

        let err = result.unwrap_err();
        assert!(err.to_string().contains("required field `TOKEN`"));
        let ws = make_workspace(&tmp);
        let conn = runs::open(&ws).unwrap();
        assert!(runs::get_run(&conn, "rid-missing-secret")
            .unwrap()
            .is_none());
    }

    // A --env-file path the user passed that does not exist is a hard error
    // (not silently ignored). The non-JSON surface returns an Err.
    #[test]
    #[cfg(unix)]
    fn test_run_missing_env_file_is_error() {
        let tmp = TempDir::new().unwrap();
        let script = write_schema_script(&tmp, "ok.sh", r#"{"Name":"Ok","Fields":[]}"#, "true");

        let result = run(
            tmp.path().to_path_buf(),
            RunArgs {
                script: script.to_string_lossy().to_string(),
                actor: "human".into(),
                reason: None,
                run_id: Some("rid-missing-envfile".into()),
                parent_run_id: None,
                no_prompt: false,
                env_file: Some(tmp.path().join("nope.env")),
                secrets: vec![],
                args: vec![],
            },
            false,
        );

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("nope.env"),
            "error should name the missing env-file path, got: {}",
            err
        );
        let ws = make_workspace(&tmp);
        let conn = runs::open(&ws).unwrap();
        assert!(runs::get_run(&conn, "rid-missing-envfile")
            .unwrap()
            .is_none());
    }
}
