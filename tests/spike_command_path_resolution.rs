//! Spike (task 1751): de-risk the venv-via-PATH thesis before any env-injection
//! code is built.
//!
//! # The risk
//!
//! The env-injection design assumes that setting `PATH` in the child environment
//! makes `std::process::Command::new("python3")` resolve to the venv's `python3`.
//! If Rust std resolved the *program name* against the **parent** process `PATH`
//! (ignoring the child's `.env("PATH", ...)` override), injection would prefill
//! the environment but silently keep running the **system** interpreter — the
//! worst kind of failure: no error, wrong imports.
//!
//! # Empirical finding (Linux, rustc 1.94.1)
//!
//! `.env("PATH", ...)` **DOES** affect program-name resolution on this platform:
//! a shim `python3` placed on a `PATH`-prepended temp dir is the binary that
//! actually executes, and a `PATH` that omits every real interpreter directory
//! makes the spawn fail with `NotFound` — proving the **child** `PATH` (not the
//! parent's) drives the lookup. See `env_path_redirects_program_name_resolution`.
//!
//! # Locked decision (for downstream task 1755)
//!
//! Regardless of the spike outcome, downstream interpreter spawning MUST resolve
//! the interpreter to an **ABSOLUTE path** via a which-style lookup against the
//! merged/injected `PATH`, then `Command::new(abs_path)`.
//!
//! Rationale:
//! - The name-resolution-honors-child-`PATH` behavior is a std implementation
//!   detail, is **not** contractually guaranteed, and differs across platforms
//!   (notably Windows). Relying on it is fragile.
//! - Absolute-path resolution is explicit and deterministic: it removes the
//!   silent-wrong-interpreter footgun entirely and makes the resolved path
//!   observable/loggable.
//!
//! `absolute_path_resolution_is_deterministic` proves the robust alternative
//! runs the shim deterministically.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use omakure::{generated_executable_tempdir, write_generated_executable};

const MARKER: &str = "SPIKE_SHIM_MARKER";

/// Write an executable shim named `python3` into `dir` that prints [`MARKER`]
/// and ignores its arguments, so a marker in stdout unambiguously means the
/// shim (not the system interpreter) ran.
fn write_python3_shim(dir: &Path) -> PathBuf {
    let final_path = dir.join("python3");
    let body = format!("#!/bin/sh\necho {MARKER}\n");
    write_generated_executable(&final_path, body.as_bytes())
        .unwrap_or_else(|error| panic!("write python3 fixture: {error}"));
    final_path
}

/// Minimal which-style lookup: return the first executable file named `program`
/// found by scanning the colon-separated `path` string left-to-right. This is a
/// spike-local stand-in for the resolver task 1755 will implement in `runtime`.
fn which_in_path(program: &str, path: &str) -> Option<PathBuf> {
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let candidate = Path::new(dir).join(program);
        if let Ok(meta) = fs::metadata(&candidate) {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return Some(candidate);
            }
        }
    }
    None
}

/// EMPIRICAL SPIKE: does `.env("PATH", ...)` change how `Command::new("python3")`
/// resolves the program name?
///
/// Two sub-assertions, both against `Command::new("python3")` (program *name*,
/// no slash — the case that forces a `PATH` search):
///  1. child `PATH` = `<shim_dir>` -> the shim runs (stdout carries the marker),
///     proving the child env override is honored and the shim beat any system
///     `python3`.
///  2. child `PATH` = `<empty_dir>` (contains no `python3`, and excludes every
///     real interpreter dir) -> the spawn fails with `NotFound`, proving the
///     **parent** `PATH` is NOT consulted as a fallback.
#[test]
fn env_path_redirects_program_name_resolution() {
    let shim_dir = generated_executable_tempdir().unwrap();
    write_python3_shim(shim_dir.path());

    // (1) shim dir on the child PATH -> the shim executes.
    let out = Command::new("python3")
        .env("PATH", shim_dir.path())
        .arg("-c")
        .arg("print('would-be-system-python')")
        .output()
        .expect("spawn with shim on child PATH should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(MARKER),
        "expected the shim to run (child PATH honored), got stdout={stdout:?} status={:?}",
        out.status.code()
    );

    // (2) child PATH that contains no python3 and no system dirs -> NotFound.
    let empty_dir = tempfile::tempdir().unwrap();
    let err = Command::new("python3")
        .env("PATH", empty_dir.path())
        .output()
        .expect_err("spawn must fail: child PATH has no python3 and parent PATH is not consulted");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "child PATH drives resolution; parent PATH must not be a fallback"
    );
}

/// LOCKED STRATEGY: resolve the interpreter to an absolute path via which-style
/// lookup against the merged/injected `PATH`, then `Command::new(abs_path)`.
/// This must run the shim deterministically without relying on any name-lookup
/// behavior of `Command`.
#[test]
fn absolute_path_resolution_is_deterministic() {
    let shim_dir = generated_executable_tempdir().unwrap();
    let shim = write_python3_shim(shim_dir.path());

    // Simulate the injected/merged PATH the way task 1755 will: venv/shim dir
    // prepended to the inherited PATH.
    let inherited = std::env::var("PATH").unwrap_or_default();
    let merged = format!("{}:{}", shim_dir.path().display(), inherited);

    let resolved = which_in_path("python3", &merged).expect("resolver must find the shim");
    assert_eq!(
        resolved, shim,
        "resolver must pick the prepended shim first"
    );
    assert!(
        resolved.is_absolute(),
        "resolved interpreter must be absolute"
    );

    let out = Command::new(&resolved)
        .output()
        .expect("spawning the absolute path must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(MARKER),
        "absolute-path spawn must run the shim deterministically, got {stdout:?}"
    );
}
