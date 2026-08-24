//! Black-box coverage for the deterministic no-subcommand contract.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn omakure_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omakure"))
}

fn run(args: &[&str]) -> Output {
    Command::new(omakure_bin())
        .args(args)
        .output()
        .expect("spawn omakure")
}

#[test]
fn no_subcommand_prints_normal_clap_help_without_startup() {
    let output = run(&[]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("Usage: omakure"));
    assert!(stdout.contains("Commands:"));
    assert!(stdout.contains("--scripts-dir"));
    assert!(output.stderr.is_empty());
}

#[test]
fn no_subcommand_json_returns_one_invalid_argument_envelope() {
    let output = run(&["--json"]);

    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);

    let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON error envelope");
    assert_eq!(envelope["ok"], false);
    assert!(envelope["data"].is_null());
    assert_eq!(envelope["error"]["code"], "invalid_argument");
    assert!(envelope["error"]["message"].is_string());
    assert_eq!(envelope["schema_version"], "1");
}
