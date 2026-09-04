use crate::adapters::system_checks::{
    ensure_bash_installed_with_env, ensure_git_installed_with_env, ensure_jq_installed_with_env,
    ensure_powershell_installed_with_env, ensure_python_installed_with_env,
};
use crate::error::{AppResult, ScriptError};
use crate::runtime::{command_for_script_with_env, script_kind, ScriptKind};
use std::path::Path;
use std::process::{Command, Stdio};

const API_TOKEN_ENV: &str = "OMAKURE_API_TOKEN";

pub struct MultiScriptRunner;

impl MultiScriptRunner {
    /// Build a [`Command`] that will execute `script` with `args` and the
    /// supplied extra environment variables. Used by the worker / inline
    /// run path so the spawned child process always carries the
    /// `OMAKURE_RUN_ID` (and any future) environment values.
    ///
    /// The worker needs to spawn the child without consuming the caller
    /// thread, monitor it for cancellation/timeout, and capture stdout and
    /// stderr after the fact.
    pub fn build_command(
        script: &Path,
        args: &[String],
        env: &[(String, String)],
    ) -> AppResult<Command> {
        ensure_runtime_for(script, env)?;
        // Resolve the interpreter against the injected PATH (if any) so a
        // venv-prepended PATH runs the venv interpreter, not the system one.
        let mut cmd = command_for_script_with_env(script, env)?;
        cmd.args(args);
        cmd.env_remove(API_TOKEN_ENV);
        for (k, v) in env {
            if k != API_TOKEN_ENV {
                cmd.env(k, v);
            }
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        Ok(cmd)
    }
}

fn ensure_runtime_for(script: &Path, env: &[(String, String)]) -> AppResult<()> {
    match script_kind(script).ok_or(ScriptError::UnsupportedType)? {
        ScriptKind::Bash => {
            ensure_git_installed_with_env(env)?;
            ensure_bash_installed_with_env(env)?;
            ensure_jq_installed_with_env(env)?;
        }
        ScriptKind::PowerShell => {
            ensure_powershell_installed_with_env(env)?;
        }
        // Nothing to ensure: the Lua runtime is compiled into this binary, so
        // there is no host dependency that could be missing.
        ScriptKind::Lua => {}
        ScriptKind::Python => {
            ensure_python_installed_with_env(env)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_build_command_bash() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.sh");
        fs::write(&script, "#!/bin/bash\necho hello").unwrap();

        let cmd = MultiScriptRunner::build_command(&script, &[], &[]).unwrap();
        #[cfg(windows)]
        assert!(cmd
            .get_program()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(r"\bash.exe"));
        #[cfg(not(windows))]
        assert_eq!(cmd.get_program(), "bash");
    }

    #[test]
    fn test_build_command_python() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.py");
        fs::write(&script, "print('hello')").unwrap();

        let cmd = MultiScriptRunner::build_command(&script, &[], &[]).unwrap();
        assert_eq!(cmd.get_program(), crate::runtime::python_program());
    }

    #[test]
    fn test_build_command_with_args_and_env() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.sh");
        fs::write(&script, "#!/bin/bash\necho $1").unwrap();

        let args = vec!["arg1".to_string()];
        let env = vec![("MY_VAR".to_string(), "my_value".to_string())];
        let cmd = MultiScriptRunner::build_command(&script, &args, &env).unwrap();

        let cmd_args: Vec<_> = cmd.get_args().collect();
        assert!(cmd_args.iter().any(|a| *a == "arg1"));

        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs
            .iter()
            .any(|(k, v)| *k == "MY_VAR" && *v == Some(std::ffi::OsStr::new("my_value"))));
    }

    #[test]
    fn test_build_command_removes_api_token() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.sh");
        fs::write(&script, "#!/bin/bash\necho ok").unwrap();

        let env = vec![(API_TOKEN_ENV.to_string(), "secret".to_string())];
        let cmd = MultiScriptRunner::build_command(&script, &[], &env).unwrap();

        assert!(cmd
            .get_envs()
            .any(|(k, v)| k == API_TOKEN_ENV && v.is_none()));
    }

    #[test]
    fn test_build_command_unsupported_type() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("test.txt");
        fs::write(&script, "hello").unwrap();

        let result = MultiScriptRunner::build_command(&script, &[], &[]);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_build_command_resolves_python_against_injected_path() {
        // Shim `python3` on an injected PATH must become the command program
        // as an absolute path, proving build_command threads env into
        // interpreter resolution (task 1755 wiring).
        let shim_dir = TempDir::new().unwrap();
        crate::adapters::system_checks::write_test_executable_shim(shim_dir.path(), "python3");

        let script_dir = TempDir::new().unwrap();
        let script = script_dir.path().join("job.py");
        fs::write(&script, "print('x')").unwrap();

        let inherited = std::env::var("PATH").unwrap_or_default();
        let injected = format!("{}:{}", shim_dir.path().display(), inherited);
        let env = vec![("PATH".to_string(), injected)];

        let cmd = MultiScriptRunner::build_command(&script, &[], &env).unwrap();
        assert_eq!(
            cmd.get_program(),
            shim_dir.path().join("python3").as_os_str()
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_build_command_rejects_interpreter_omitted_from_injected_path() {
        let empty_dir = TempDir::new().unwrap();
        let script_dir = TempDir::new().unwrap();
        let script = script_dir.path().join("job.py");
        fs::write(&script, "print('x')").unwrap();
        let env = vec![("PATH".to_string(), empty_dir.path().display().to_string())];

        let result = MultiScriptRunner::build_command(&script, &[], &env);
        assert!(matches!(
            result,
            Err(crate::error::AppError::Script(
                ScriptError::DependencyMissing { name, .. }
            )) if name == crate::runtime::python_program()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_build_command_resolves_bash_against_injected_path() {
        let bin_dir = TempDir::new().unwrap();
        for program in ["bash", "git", "jq"] {
            crate::adapters::system_checks::write_test_executable_shim(bin_dir.path(), program);
        }

        let script_dir = TempDir::new().unwrap();
        let script = script_dir.path().join("job.sh");
        fs::write(&script, "echo ok").unwrap();
        let env = vec![("PATH".to_string(), bin_dir.path().display().to_string())];

        let command = MultiScriptRunner::build_command(&script, &[], &env).unwrap();
        assert_eq!(
            command.get_program(),
            bin_dir.path().join("bash").as_os_str()
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_build_command_resolves_powershell_against_injected_path() {
        let bin_dir = TempDir::new().unwrap();
        let program = crate::runtime::powershell_program();
        crate::adapters::system_checks::write_test_executable_shim(bin_dir.path(), program);

        let script_dir = TempDir::new().unwrap();
        let script = script_dir.path().join("job.ps1");
        fs::write(&script, "Write-Output ok").unwrap();
        let env = vec![("PATH".to_string(), bin_dir.path().display().to_string())];

        let command = MultiScriptRunner::build_command(&script, &[], &env).unwrap();
        assert_eq!(
            command.get_program(),
            bin_dir.path().join(program).as_os_str()
        );
    }
}
