mod support;

#[cfg(unix)]
#[test]
fn failing_run_keeps_script_exit_code_and_output_in_both_cli_modes() {
    let workspace = support::TestWorkspace::new("run_exit");
    let script = workspace.write_schema_script(
        "fail.sh",
        "fail_fixture",
        "printf 'stdout-fragment'; printf 'stderr-fragment' >&2; exit 7",
    );
    support::set_executable(&script);

    let human = support::workspace_command::<20>(workspace.path(), &["run", "fail.sh"]);
    assert_eq!(human.status.code(), Some(7));
    assert_eq!(human.stdout, b"stdout-fragment\n");
    assert_eq!(human.stderr, b"stderr-fragment\n");

    let json = support::workspace_command::<20>(workspace.path(), &["--json", "run", "fail.sh"]);
    assert_eq!(json.status.code(), Some(7));
    assert!(json.stderr.is_empty());
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["data"]["state"], "failed");
    assert_eq!(envelope["data"]["stdout"], "stdout-fragment\n");
    assert_eq!(envelope["data"]["stderr"], "stderr-fragment\n");
}
