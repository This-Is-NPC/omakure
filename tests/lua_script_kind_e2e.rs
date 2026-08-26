//! Behaviour of the embedded Lua script kind, exercised through the real
//! `omakure` binary.
//!
//! These live in `tests/` rather than in-crate on purpose. Cargo sets
//! `CARGO_BIN_EXE_omakure` only for `tests/` and `benches/` targets, so an
//! in-crate test would resolve `current_exe()` to the test harness instead of
//! `omakure` and would prove nothing about the self-exec path.
//!
//! Scope is deliberately narrow: only what *differs* from the existing script
//! kinds. `run_executor::execute_with_heartbeat` cannot tell one
//! `std::process::Command` from another, so re-proving cancel, heartbeat,
//! redaction, or the queue-worker path for Lua would be theatre.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn omakure() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omakure"))
}

/// Invoke the embedded host directly, the way `command_for_script_with_env`
/// spawns it.
fn host(script: &Path, args: &[&str]) -> Output {
    Command::new(omakure())
        .arg("--__omakure-lua-host")
        .arg(script)
        .args(args)
        .output()
        .expect("run the embedded Lua host")
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write the script");
    path
}

#[test]
fn a_lua_script_runs_and_captures_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let script = write(dir.path(), "hello.lua", "print('from lua')\n");

    let output = host(&script, &[]);

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "from lua");
}

/// The highest-risk silent bug in this kind.
///
/// `--scripts-dir` and `--json` are declared `global = true` on omakure's own
/// parser. Had the host been a clap subcommand, omakure would have consumed a
/// script's own flags and `--help` would have printed omakure's help and exited
/// zero. The argv intercept is what makes that impossible, so it is asserted on
/// exactly the arguments that would have been swallowed.
#[test]
fn hyphen_prefixed_arguments_reach_lua_byte_identically() {
    let dir = tempfile::tempdir().unwrap();
    let script = write(dir.path(), "args.lua", "print(table.concat(arg, '\\30'))\n");

    let forwarded = ["--json", "--scripts-dir=x", "--help", "-h", "--", "-"];
    let output = host(&script, &forwarded);

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let seen: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .split('\u{1e}')
        .map(str::to_string)
        .collect();
    assert_eq!(seen, forwarded, "arguments must arrive unmodified");
}

/// `arg[0]` is the script path, as it is for the standalone interpreter.
#[test]
fn arg_zero_is_the_script_path() {
    let dir = tempfile::tempdir().unwrap();
    let script = write(dir.path(), "argzero.lua", "print(arg[0])\n");

    let output = host(&script, &[]);

    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        script.to_string_lossy()
    );
}

#[test]
fn a_lua_runtime_error_exits_non_zero_with_the_message_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let script = write(dir.path(), "boom.lua", "error('deliberate failure')\n");

    let output = host(&script, &[]);

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "the message must not reach stdout"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("deliberate failure"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A script's own exit code must stay distinguishable from a host failure, or
/// an operator cannot tell "the script decided to fail" from "the script never
/// ran".
#[test]
fn the_scripts_own_exit_code_survives() {
    let dir = tempfile::tempdir().unwrap();
    let script = write(dir.path(), "exit.lua", "os.exit(3)\n");

    assert_eq!(host(&script, &[]).status.code(), Some(3));
}

#[test]
fn a_missing_script_uses_the_reserved_host_failure_code() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.lua");

    let output = host(&missing, &[]);

    assert_eq!(
        output.status.code(),
        Some(126),
        "a host failure must not be mistaken for the script exiting 1"
    );
}

/// The point of `vendored`: a node needs no system Lua.
#[test]
fn the_host_does_not_depend_on_a_system_lua() {
    let dir = tempfile::tempdir().unwrap();
    let script = write(dir.path(), "selfcontained.lua", "print(_VERSION)\n");

    // An empty PATH removes any chance of falling back to an installed
    // interpreter; the runtime is inside the binary or this fails.
    let output = Command::new(omakure())
        .arg("--__omakure-lua-host")
        .arg(&script)
        .env("PATH", "")
        .output()
        .expect("run the embedded Lua host");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Lua 5.4"));
}

/// The one end-to-end proof that Lua rides the shared executor.
///
/// `run_executor` is `Command`-opaque, so cancel, heartbeat, capture and
/// redaction cannot behave differently for Lua than for Bash. Rather than
/// re-prove each of them, this asserts the single property that would break if
/// the self-exec command were not an ordinary child process: a per-job timeout
/// kills it.
///
/// It goes through the queue because that is where per-job timeouts live.
/// `--timeout` belongs to `queue add`, and the inline `run` path hardcodes
/// `timeout_ms: None` (`src/cli/run.rs`), so `omakure run x.lua --timeout 2s`
/// would prove nothing for any script kind.
#[test]
fn a_per_job_timeout_kills_a_running_lua_script() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "sleeper.lua",
        // Busy-wait rather than sleep: Lua has no portable sleep, and the point
        // is only that the process is genuinely alive when the deadline lands.
        "local deadline = os.time() + 60\nwhile os.time() < deadline do end\n",
    );

    let enqueue = Command::new(omakure())
        .current_dir(dir.path())
        .args([
            "queue",
            "add",
            "sleeper",
            "--timeout",
            "2s",
            "--scripts-dir",
        ])
        .arg(dir.path())
        .output()
        .expect("enqueue the job");
    assert!(
        enqueue.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&enqueue.stderr)
    );

    let started = std::time::Instant::now();
    let worker = Command::new(omakure())
        .current_dir(dir.path())
        .args(["queue", "worker", "--once", "--scripts-dir"])
        .arg(dir.path())
        .output()
        .expect("drain the queue");
    let elapsed = started.elapsed();
    assert!(
        worker.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&worker.stderr)
    );
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "the kill never landed; draining took {elapsed:?} against a 2s timeout \
         on a script that would otherwise run for 60s"
    );

    let stats = Command::new(omakure())
        .current_dir(dir.path())
        .args(["queue", "stats", "--json", "--scripts-dir"])
        .arg(dir.path())
        .output()
        .expect("read queue stats");
    let stats = String::from_utf8_lossy(&stats.stdout);
    assert!(
        stats.contains("\"timed_out\":1"),
        "the run should be recorded as timed out, got: {stats}"
    );
}
