use crate::adapters::system_checks::{
    ensure_bash_installed, ensure_git_installed, ensure_jq_installed, ensure_powershell_installed,
    ensure_python_installed,
};
use crate::error::{AppResult, ScriptError};
use crate::ports::{ScriptRunOutput, ScriptRunner};
use crate::runtime::{command_for_script, script_kind, ScriptKind};
use std::path::Path;
use std::process::{Command, Stdio};

pub struct MultiScriptRunner;

impl MultiScriptRunner {
    pub fn new() -> Self {
        Self
    }

    /// Build a [`Command`] that will execute `script` with `args` and the
    /// supplied extra environment variables. Used by the worker / inline
    /// run path so the spawned child process always carries the
    /// `OMAKURE_RUN_ID` (and any future) environment values.
    ///
    /// This bypasses the `ScriptRunner` trait's `output()`-and-block API
    /// because the worker needs to spawn the child without consuming the
    /// caller thread, monitor it for cancellation/timeout, and capture
    /// stdout/stderr after the fact.
    pub fn build_command(
        script: &Path,
        args: &[String],
        env: &[(String, String)],
    ) -> AppResult<Command> {
        ensure_runtime_for(script)?;
        let mut cmd = command_for_script(script)?;
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        Ok(cmd)
    }
}

fn ensure_runtime_for(script: &Path) -> AppResult<()> {
    match script_kind(script).ok_or(ScriptError::UnsupportedType)? {
        ScriptKind::Bash => {
            ensure_git_installed()?;
            ensure_bash_installed()?;
            ensure_jq_installed()?;
        }
        ScriptKind::PowerShell => {
            ensure_powershell_installed()?;
        }
        ScriptKind::Python => {
            ensure_python_installed()?;
        }
    }
    Ok(())
}

impl ScriptRunner for MultiScriptRunner {
    fn run(&self, script: &Path, args: &[String]) -> AppResult<ScriptRunOutput> {
        ensure_runtime_for(script)?;

        let output = command_for_script(script)?.args(args).output()?;
        Ok(ScriptRunOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            success: output.status.success(),
        })
    }
}
