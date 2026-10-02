pub mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn unique_temp(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("omakure_battery_test_{label}_{pid}_{nanos}"))
}

fn run_json_battery(workspace: &Path, args: &[&str]) -> Output {
    Command::new(support::omakure_bin())
        .arg("--scripts-dir")
        .arg(workspace)
        .arg("--json")
        .arg("battery")
        .args(args)
        .output()
        .expect("spawn omakure")
}

#[cfg(unix)]
#[test]
fn installed_workflow_runs_in_order_and_reports_provenance_and_failure() {
    let workspace = support::TestWorkspace::new("cli_battery_workflow");
    let repo = support::TestWorkspace::new("cli_battery_workflow_repo");
    support::write_local_battery_repo(repo.path(), "local", "Workflow fixture");

    let manifest_path = repo.path().join("omakure-battery.toml");
    let mut manifest = std::fs::read_to_string(&manifest_path).expect("read Battery manifest");
    manifest.push_str(
        r#"
[[scripts]]
id = "local.finish"
path = "scripts/finish.sh"

[[workflows]]
id = "local.sequence"
scripts = ["local.echo", "local.finish"]
"#,
    );
    std::fs::write(&manifest_path, manifest).expect("write workflow manifest");
    let finish = repo.path().join("scripts/finish.sh");
    std::fs::write(
        &finish,
        "#!/bin/sh\n# OMAKURE_SCHEMA_START\n# {\"Name\":\"Battery Finish\",\"Fields\":[]}\n# OMAKURE_SCHEMA_END\necho finish\n",
    )
    .expect("write second script");
    support::set_executable(&finish);
    for args in [
        &["add", "."][..],
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "workflow fixture",
        ][..],
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?} failed");
    }

    for args in [
        &[
            "add",
            repo.path().to_str().expect("repo path"),
            "--name",
            "local",
        ][..],
        &["sync", "local"][..],
        &["install", "local", "local.echo"][..],
        &["install", "local", "local.finish"][..],
    ] {
        let output = run_json_battery(workspace.path(), args);
        assert!(
            output.status.success(),
            "battery {args:?} failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    let started = run_json_battery(
        workspace.path(),
        &["workflow", "start", "local", "local.sequence"],
    );
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stdout)
    );
    let started: serde_json::Value = serde_json::from_slice(&started.stdout).expect("start JSON");
    let id = started["data"]["workflow_id"]
        .as_str()
        .expect("workflow ID");
    let commit = started["data"]["battery_commit"]
        .as_str()
        .expect("Battery commit");
    assert_eq!(started["data"]["battery_version"], "0.1.0");
    assert_eq!(commit.len(), 40);
    assert_eq!(started["data"]["steps"].as_array().unwrap().len(), 2);

    let status = run_json_battery(workspace.path(), &["workflow", "status", id]);
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status JSON");
    assert_eq!(status["data"]["workflow_id"], id);
    assert_eq!(status["data"]["battery_version"], "0.1.0");
    assert_eq!(status["data"]["battery_commit"], commit);
    assert_eq!(status["data"]["steps"][0]["state"], "queued");
    assert!(status["data"]["steps"][1]["run_id"].is_null());

    let worker_once = || {
        let output = support::workspace_command::<30>(
            workspace.path(),
            &["--json", "queue", "worker", "--once"],
        );
        assert!(
            output.status.success(),
            "worker failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let status_for = |workflow_id: &str| {
        let output = run_json_battery(workspace.path(), &["workflow", "status", workflow_id]);
        assert!(
            output.status.success(),
            "status failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("status JSON")
    };

    worker_once();
    let first_done = status_for(id);
    assert_eq!(first_done["data"]["state"], "running");
    assert_eq!(first_done["data"]["steps"][0]["state"], "completed");
    assert_eq!(first_done["data"]["steps"][1]["state"], "queued");
    assert!(first_done["data"]["steps"][1]["run_id"].is_string());

    worker_once();
    let done = status_for(id);
    assert_eq!(done["data"]["state"], "completed");
    assert_eq!(done["data"]["steps"][0]["state"], "completed");
    assert_eq!(done["data"]["steps"][1]["state"], "completed");
    assert_ne!(
        done["data"]["steps"][0]["run_id"],
        done["data"]["steps"][1]["run_id"]
    );

    let human = Command::new(support::omakure_bin())
        .arg("--scripts-dir")
        .arg(workspace.path())
        .args(["battery", "workflow", "status", id])
        .output()
        .expect("run workflow status");
    assert!(human.status.success());
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("version 0.1.0"));
    assert!(text.contains(&format!("commit {commit}")));

    let changed = run_json_battery(
        workspace.path(),
        &["workflow", "start", "local", "local.sequence"],
    );
    assert!(changed.status.success());
    let changed: serde_json::Value =
        serde_json::from_slice(&changed.stdout).expect("second start JSON");
    let changed_id = changed["data"]["workflow_id"]
        .as_str()
        .expect("second workflow ID");
    worker_once();
    let marker = workspace.path().join("changed-step-ran");
    let finish = workspace.path().join("scripts/finish.sh");
    let mut content = std::fs::read(&finish).expect("read installed script");
    content.extend_from_slice(format!("echo changed > {}\n", marker.display()).as_bytes());
    std::fs::write(&finish, content).expect("change installed script");
    worker_once();

    let failed = status_for(changed_id);
    assert_eq!(failed["data"]["state"], "failed");
    assert_eq!(failed["data"]["steps"][0]["state"], "completed");
    assert_eq!(failed["data"]["steps"][1]["state"], "failed");
    assert!(failed["data"]["steps"][1]["run_id"].is_string());
    assert!(failed["data"]["steps"][1]["error"].as_str().is_some());
    assert!(!marker.exists(), "changed script must not execute");
}

#[test]
fn json_battery_errors_emit_single_stdout_envelope_without_stderr() {
    let dir = unique_temp("json_error");
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let output = run_json_battery(&dir, &["inspect", "missing"]);

    let _ = std::fs::remove_dir_all(&dir);

    assert!(!output.status.success(), "expected non-zero exit");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "expected one JSON line, got: {stdout}");
    let envelope: serde_json::Value = serde_json::from_str(lines[0]).expect("json envelope");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "not_found");
}

#[test]
fn workflow_start_and_status_report_missing_resources_in_json() {
    let dir = unique_temp("workflow_missing");
    std::fs::create_dir_all(&dir).expect("create temp dir");

    for args in [
        &["workflow", "start", "missing", "deploy"][..],
        &["workflow", "status", "missing-run"][..],
    ] {
        let output = run_json_battery(&dir, args);
        assert!(
            !output.status.success(),
            "expected non-zero exit for {args:?}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stderr), "");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 1, "expected one JSON line, got: {stdout}");
        let envelope: serde_json::Value = serde_json::from_str(lines[0]).expect("JSON envelope");
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "not_found");
    }

    let _ = std::fs::remove_dir_all(&dir);
}
