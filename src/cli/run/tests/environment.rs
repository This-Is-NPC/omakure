use super::*;
use pretty_assertions::assert_eq;

// CALL SITE: `omakure run` (cli/run/mod.rs). The active managed env must
// reach the spawned process; the script echoes an injected var and the
// value must land in the persisted run record's stdout.
#[test]
#[cfg(unix)]
fn test_run_injects_active_env_into_script_and_persists_output() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
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
