//! End-to-end CLI tests for the positional scripts-root override
//! introduced by the "open TUI from current directory" feature.
//!
//! These tests drive the compiled `omakure` binary as a subprocess and
//! assert exit codes / stderr messages without launching an actual
//! interactive TUI. They cover the three failure modes (nonexistent
//! path, file-not-a-directory, conflict with `--scripts-dir`) plus the
//! regression case that existing subcommand parsing still works when a
//! positional argument is or is not present.
//!
//! Acceptance criteria covered:
//! - Nonexistent / not-a-directory / conflict cases each exit with a
//!   clean, deterministic error and do not start the TUI.
//! - `omakure run ...`, `omakure scripts`, etc. still parse exactly as
//!   before and are unaffected by the new positional argument.

use std::path::PathBuf;
use std::process::Command;

fn omakure_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omakure"))
}

fn unique_temp(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("omakure_test_{label}_{pid}_{nanos}"))
}

#[test]
fn nonexistent_positional_path_exits_with_clear_error() {
    let missing = unique_temp("missing");
    let _ = std::fs::remove_dir_all(&missing);

    let output = Command::new(omakure_bin())
        .arg(&missing)
        .output()
        .expect("spawn omakure");

    assert!(!output.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("scripts directory not found"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains(&missing.display().to_string()),
        "stderr should include the missing path: {stderr}"
    );
}

#[test]
fn positional_path_pointing_at_a_file_exits_with_clear_error() {
    let file = unique_temp("file");
    std::fs::write(&file, "not a directory").expect("create temp file");

    let output = Command::new(omakure_bin())
        .arg(&file)
        .output()
        .expect("spawn omakure");

    let _ = std::fs::remove_file(&file);

    assert!(!output.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected a directory"),
        "stderr was: {stderr}"
    );
}

#[test]
fn positional_path_conflicts_with_scripts_dir_flag() {
    let dir = unique_temp("conflict");
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let output = Command::new(omakure_bin())
        .arg(&dir)
        .arg("--scripts-dir")
        .arg(&dir)
        .output()
        .expect("spawn omakure");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        !output.status.success(),
        "expected non-zero exit when positional + --scripts-dir are combined"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Clap renders the conflict in the standard "cannot be used with"
    // phrasing. Accept either of clap's wording variants.
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("conflicts"),
        "stderr was: {stderr}"
    );
}

#[test]
fn run_subcommand_without_positional_path_is_unaffected() {
    // The `run` subcommand still expects a script name. Without one it
    // should fail with clap's "missing argument" error, *not* with the
    // positional path validation error — this proves the positional
    // argument doesn't shadow subcommand parsing.
    let output = Command::new(omakure_bin())
        .arg("run")
        .output()
        .expect("spawn omakure");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required") || stderr.contains("SCRIPT") || stderr.contains("Usage"),
        "expected clap missing-arg error, got: {stderr}"
    );
    assert!(
        !stderr.contains("scripts directory not found"),
        "subcommand parsing must not trigger positional-path validation"
    );
    assert!(
        !stderr.contains("expected a directory"),
        "subcommand parsing must not trigger positional-path validation"
    );
}

#[test]
fn scripts_subcommand_runs_without_positional_path() {
    // `omakure scripts` lists scripts and exits cleanly. We don't care
    // what it prints, only that it doesn't blow up because of the new
    // positional argument and that exit is success.
    let dir = unique_temp("scripts_subcmd");
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let output = Command::new(omakure_bin())
        .arg("--scripts-dir")
        .arg(&dir)
        .arg("scripts")
        .output()
        .expect("spawn omakure");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "scripts subcommand should succeed against an empty workspace; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn config_subcommand_runs_without_positional_path() {
    let dir = unique_temp("config_subcmd");
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let output = Command::new(omakure_bin())
        .arg("--scripts-dir")
        .arg(&dir)
        .arg("config")
        .output()
        .expect("spawn omakure");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "config subcommand should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
