pub mod support;

use serde_json::Value;
use std::fs;
use std::path::Path;
use support::{assert_no_plaintext, assert_success};

const RUN_SECRET: &str = "run-secret-cli-e2e-plain-value";
const ENV_TOKEN: &str = "env-token-cli-e2e-plain-value";
const ENV_API_KEY: &str = "env-api-key-cli-e2e-plain-value";
const QUEUE_SECRET: &str = "queue-secret-cli-e2e-plain-value";

#[test]
fn run_secret_json_and_history_redact_plaintext() {
    let workspace = support::TestWorkspace::new("secret_run");
    write_secret_echo_script(workspace.path(), "secret-run.sh");

    let direct_secret = format!("TOKEN={RUN_SECRET}");
    let output = support::workspace_command_with_env::<15>(
        workspace.path(),
        &["--json", "run", "secret-run.sh", "--secret", &direct_secret],
        &[("OMAKURE_EXPECTED_TOKEN", RUN_SECRET)],
    );
    assert_success(&output);
    assert_no_plaintext(&output, RUN_SECRET);

    let envelope = support::json_envelope(&output.stdout);
    assert_eq!(envelope["ok"], true);
    let run_id = envelope["data"]["run_id"].as_str().expect("run id");
    assert_eq!(envelope["data"]["stdout"], "script-saw-redacted-ok\n");
    assert_redacted_run_row(&envelope["data"], RUN_SECRET);

    let history =
        support::workspace_command::<15>(workspace.path(), &["--json", "history", "show", run_id]);
    assert_success(&history);
    assert_no_plaintext(&history, RUN_SECRET);
    let history_envelope = support::json_envelope(&history.stdout);
    assert_eq!(history_envelope["ok"], true);
    assert_eq!(
        history_envelope["data"]["stdout"],
        "script-saw-redacted-ok\n"
    );
    assert_redacted_run_row(&history_envelope["data"], RUN_SECRET);
}

#[test]
fn run_rejects_malformed_direct_secrets_without_echoing_values() {
    let workspace = support::TestWorkspace::new("invalid_direct_secret");
    write_secret_echo_script(workspace.path(), "secret-run.sh");

    for (argument, expected_message) in [
        (
            "raw_secret_value",
            "invalid secret argument: expected FIELD=VALUE",
        ),
        (
            " =raw_secret_value",
            "invalid secret: field name cannot be empty",
        ),
    ] {
        let output = support::workspace_command::<15>(
            workspace.path(),
            &["--json", "run", "secret-run.sh", "--secret", argument],
        );
        assert!(!output.status.success());
        assert_no_plaintext(&output, "raw_secret_value");

        let envelope = support::json_envelope(&output.stdout);
        assert_eq!(envelope["error"]["code"], "invalid_argument");
        assert_eq!(envelope["error"]["message"], expected_message);
    }
}

#[test]
fn env_lifecycle_masks_sensitive_values_without_stdout_or_stderr_leaks() {
    let workspace = support::TestWorkspace::new("secret_env");
    let api_key_param = format!("API_KEY={ENV_API_KEY}");
    let token_param = format!("TOKEN={ENV_TOKEN}");

    for args in [
        vec![
            "--json",
            "env",
            "create",
            "prod",
            api_key_param.as_str(),
            token_param.as_str(),
            "HOST=prod.example.test",
        ],
        vec!["--json", "env", "show", "prod"],
        vec!["--json", "env", "set", "prod", token_param.as_str()],
        vec!["--json", "env", "remove", "prod", "TOKEN"],
        vec!["--json", "env", "activate", "prod"],
        vec!["--json", "env", "deactivate"],
    ] {
        let output = support::workspace_command::<15>(workspace.path(), &args);
        assert_success(&output);
        assert_no_plaintext(&output, ENV_TOKEN);
        assert_no_plaintext(&output, ENV_API_KEY);
    }

    let show =
        support::workspace_command::<15>(workspace.path(), &["--json", "env", "show", "prod"]);
    assert_success(&show);
    assert_no_plaintext(&show, ENV_TOKEN);
    assert_no_plaintext(&show, ENV_API_KEY);
    let envelope = support::json_envelope(&show.stdout);
    let entries = envelope["data"].as_array().expect("env entries");
    let api_key = entries
        .iter()
        .find(|entry| entry["key"] == "API_KEY")
        .expect("API_KEY entry");
    assert_eq!(api_key["value"], "****");
}

#[test]
fn queue_worker_and_history_redact_reconstructable_secret_refs() {
    let workspace = support::TestWorkspace::new("secret_queue");
    write_secret_echo_script(workspace.path(), "secret-queue.sh");

    let add = support::workspace_command_with_env::<15>(
        workspace.path(),
        &[
            "--json",
            "queue",
            "add",
            "secret-queue.sh",
            "--",
            "--token",
            "secret://env/OMAKURE_QUEUE_E2E_TOKEN",
        ],
        &[("OMAKURE_QUEUE_E2E_TOKEN", QUEUE_SECRET)],
    );
    assert_success(&add);
    assert_no_plaintext(&add, QUEUE_SECRET);
    let add_envelope = support::json_envelope(&add.stdout);
    let run_id = add_envelope["data"]["run_id"].as_str().expect("run id");
    assert_eq!(add_envelope["data"]["state"], "queued");
    assert_redacted_run_row(&add_envelope["data"], QUEUE_SECRET);

    let worker = support::workspace_command_with_env::<15>(
        workspace.path(),
        &["--json", "queue", "worker", "--once"],
        &[
            ("OMAKURE_QUEUE_E2E_TOKEN", QUEUE_SECRET),
            ("OMAKURE_EXPECTED_TOKEN", QUEUE_SECRET),
        ],
    );
    assert_success(&worker);
    assert_no_plaintext(&worker, QUEUE_SECRET);

    let history =
        support::workspace_command::<15>(workspace.path(), &["--json", "history", "show", run_id]);
    assert_success(&history);
    assert_no_plaintext(&history, QUEUE_SECRET);
    let history_envelope = support::json_envelope(&history.stdout);
    assert_eq!(history_envelope["data"]["state"], "completed");
    assert_eq!(history_envelope["data"]["success"], true);
    assert_eq!(
        history_envelope["data"]["stdout"],
        "script-saw-redacted-ok\n"
    );
    assert_redacted_run_row(&history_envelope["data"], QUEUE_SECRET);
}

#[test]
fn queue_add_rejects_plaintext_secret_args_that_workers_cannot_reconstruct() {
    let workspace = support::TestWorkspace::new("secret_queue_reject");
    write_secret_echo_script(workspace.path(), "secret-queue.sh");

    let output = support::workspace_command::<15>(
        workspace.path(),
        &[
            "--json",
            "queue",
            "add",
            "secret-queue.sh",
            "--",
            "--token",
            QUEUE_SECRET,
        ],
    );
    assert!(!output.status.success(), "expected queue add rejection");
    assert_no_plaintext(&output, QUEUE_SECRET);

    let envelope = support::json_envelope(&output.stdout);
    assert_eq!(envelope["ok"], false);
    let message = envelope["error"]["message"]
        .as_str()
        .expect("error message");
    assert!(
        message.contains("secret://") || message.contains("reconstruct"),
        "unexpected error message: {message}"
    );
}

fn write_secret_echo_script(workspace: &Path, name: &str) {
    let script = workspace.join(name);
    fs::write(
        &script,
        r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {
#   "Name": "secret_echo",
#   "Description": "secret e2e fixture",
#   "Fields": [
#     {"Name":"TOKEN","Prompt":"Token","Type":"secret","Required":true,"Arg":"--token"}
#   ]
# }
# OMAKURE_SCHEMA_END
if [ "$2" = "$OMAKURE_EXPECTED_TOKEN" ]; then
  printf 'script-saw-redacted-ok\n'
else
  printf 'secret mismatch\n' >&2
  exit 42
fi
"#,
    )
    .expect("write secret script");
    support::set_executable(&script);
}

fn assert_redacted_run_row(row: &Value, secret: &str) {
    let serialized = serde_json::to_string(row).expect("serialize run row");
    support::assert_redacted(&serialized, secret);

    let args_json = row["args_json"].as_str().expect("args_json string");
    support::assert_redacted(args_json, secret);
    assert!(
        args_json.contains("<redacted>") || args_json.contains("secret://"),
        "stored args must contain a redaction marker or provider ref: {args_json}"
    );
}
