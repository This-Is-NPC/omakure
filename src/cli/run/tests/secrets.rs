use super::*;
use pretty_assertions::assert_eq;

#[test]
#[cfg(unix)]
fn test_run_resolves_secret_from_env_file_arg_and_redacts_persistence() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);

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
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
    let conn = runs::open(&ws).unwrap();
    assert!(
        runs::get_run(&conn, "rid-missing-secret")
            .unwrap()
            .is_none()
    );
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
    let ws = workspace_in(&tmp);
    let conn = runs::open(&ws).unwrap();
    assert!(
        runs::get_run(&conn, "rid-missing-envfile")
            .unwrap()
            .is_none()
    );
}
