use std::ffi::OsStr;
use std::process::Command;

use crate::error::ScriptError;
#[cfg(windows)]
use crate::runtime::BASH_MISSING_HINT;
use crate::runtime::{powershell_program, python_program};

/// Check that a command is available and runs successfully using the effective
/// injected PATH. If no PATH override is supplied, preserve the normal parent
/// environment lookup.
fn ensure_command_with_env(
    program: &str,
    args: &[&str],
    not_found_hint: &str,
    env: &[(String, String)],
) -> Result<(), ScriptError> {
    let Some(injected_path) = crate::runtime::path_value(env) else {
        return ensure_command(program, args, not_found_hint);
    };
    let Some(path) = crate::runtime::resolve_program_in_path(program, injected_path) else {
        return Err(ScriptError::DependencyMissing {
            name: program.to_string(),
            hint: not_found_hint.to_string(),
        });
    };
    ensure_command_os_with_env(path, program, args, not_found_hint, env)
}

/// Check a command using an already-resolved executable and injected env.
fn ensure_command_os_with_env(
    program: impl AsRef<OsStr>,
    name: &str,
    args: &[&str],
    not_found_hint: &str,
    env: &[(String, String)],
) -> Result<(), ScriptError> {
    let mut command = Command::new(program);
    command.envs(env.iter().map(|(key, value)| (key, value)));
    ensure_command_output(command, name, args, not_found_hint)
}

/// Check a command using the parent environment.
fn ensure_command_os(
    program: impl AsRef<OsStr>,
    name: &str,
    args: &[&str],
    not_found_hint: &str,
) -> Result<(), ScriptError> {
    ensure_command_output(Command::new(program), name, args, not_found_hint)
}

/// Check a command by name using the parent environment.
fn ensure_command(program: &str, args: &[&str], not_found_hint: &str) -> Result<(), ScriptError> {
    ensure_command_os(program, program, args, not_found_hint)
}
fn ensure_command_output(
    mut command: Command,
    name: &str,
    args: &[&str],
    not_found_hint: &str,
) -> Result<(), ScriptError> {
    match command.args(args).output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim().to_string();
                Err(ScriptError::DependencyCheckFailed {
                    name: name.to_string(),
                    message: if message.is_empty() {
                        "check failed".to_string()
                    } else {
                        message
                    },
                })
            }
        }
        Err(_) => Err(ScriptError::DependencyMissing {
            name: name.to_string(),
            hint: not_found_hint.to_string(),
        }),
    }
}

#[cfg(windows)]
pub(crate) fn ensure_git_installed() -> Result<(), ScriptError> {
    ensure_command(
        "git",
        &["--version"],
        "Install Git for Windows (includes bash)",
    )
}

#[cfg(not(windows))]
pub(crate) fn ensure_git_installed() -> Result<(), ScriptError> {
    ensure_command(
        "git",
        &["--version"],
        "Install Git and ensure it is in PATH",
    )
}

pub(crate) fn ensure_git_installed_with_env(env: &[(String, String)]) -> Result<(), ScriptError> {
    let hint = if cfg!(windows) {
        "Install Git for Windows (includes bash)"
    } else {
        "Install Git and ensure it is in PATH"
    };
    ensure_command_with_env("git", &["--version"], hint, env)
}

pub(crate) fn ensure_bash_installed() -> Result<(), ScriptError> {
    ensure_bash_installed_with_env(&[])
}

#[cfg(windows)]
pub(crate) fn ensure_bash_installed_with_env(env: &[(String, String)]) -> Result<(), ScriptError> {
    let Some(program) = crate::runtime::resolve_bash_program(env) else {
        return Err(ScriptError::DependencyMissing {
            name: "bash".to_string(),
            hint: BASH_MISSING_HINT.to_string(),
        });
    };
    ensure_command_os_with_env(&program, "bash", &["--version"], BASH_MISSING_HINT, env)
}

#[cfg(not(windows))]
pub(crate) fn ensure_bash_installed_with_env(env: &[(String, String)]) -> Result<(), ScriptError> {
    let Some(injected_path) = crate::runtime::path_value(env) else {
        return ensure_command(
            "bash",
            &["--version"],
            "Install bash and ensure it is in PATH",
        );
    };
    let Some(program) = crate::runtime::resolve_program_in_path("bash", injected_path) else {
        return Err(ScriptError::DependencyMissing {
            name: "bash".to_string(),
            hint: "Install bash and ensure it is in PATH".to_string(),
        });
    };
    ensure_command_os_with_env(
        program,
        "bash",
        &["--version"],
        "Install bash and ensure it is in PATH",
        env,
    )
}

pub(crate) fn ensure_jq_installed() -> Result<(), ScriptError> {
    ensure_command("jq", &["--version"], "Install jq and ensure it is in PATH")
}

pub(crate) fn ensure_jq_installed_with_env(env: &[(String, String)]) -> Result<(), ScriptError> {
    ensure_command_with_env(
        "jq",
        &["--version"],
        "Install jq and ensure it is in PATH",
        env,
    )
}

pub(crate) fn ensure_powershell_installed() -> Result<(), ScriptError> {
    let program = powershell_program();
    ensure_command(
        program,
        &["-NoProfile", "-Command", "$PSVersionTable.PSVersion"],
        &format!("Install PowerShell and ensure {} is in PATH", program),
    )
}

pub(crate) fn ensure_powershell_installed_with_env(
    env: &[(String, String)],
) -> Result<(), ScriptError> {
    let program = powershell_program();
    let hint = format!("Install PowerShell and ensure {} is in PATH", program);
    ensure_command_with_env(
        program,
        &["-NoProfile", "-Command", "$PSVersionTable.PSVersion"],
        &hint,
        env,
    )
}

pub(crate) fn ensure_python_installed() -> Result<(), ScriptError> {
    let program = python_program();
    ensure_command(
        program,
        &["--version"],
        &format!("Install Python and ensure {} is in PATH", program),
    )
}

pub(crate) fn ensure_python_installed_with_env(
    env: &[(String, String)],
) -> Result<(), ScriptError> {
    let program = python_program();
    let hint = format!("Install Python and ensure {} is in PATH", program);
    ensure_command_with_env(program, &["--version"], &hint, env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_command_success() {
        let result = ensure_command("echo", &["hello"], "echo should exist");
        assert!(result.is_ok());
    }

    #[test]
    fn test_ensure_command_not_found() {
        let result = ensure_command(
            "this_command_does_not_exist_abc123",
            &[],
            "should not be found",
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::DependencyMissing { name, .. } => {
                assert_eq!(name, "this_command_does_not_exist_abc123");
            }
            other => panic!("expected DependencyMissing, got {:?}", other),
        }
    }

    #[test]
    fn test_ensure_command_check_failed() {
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/C", "exit /B 1"]);
        #[cfg(not(windows))]
        let (program, args) = ("false", Vec::new());

        let result = ensure_command(program, &args, "should fail");
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::DependencyCheckFailed { name, .. } => {
                assert_eq!(name, program);
            }
            other => panic!("expected DependencyCheckFailed, got {:?}", other),
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn test_ensure_bash_installed() {
        assert!(ensure_bash_installed().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn test_ensure_bash_rejects_wsl_launcher() {
        let root = tempfile::tempdir().unwrap();
        let wsl_dir = root.path().join("Windows").join("System32");
        std::fs::create_dir_all(&wsl_dir).unwrap();
        std::fs::write(wsl_dir.join("bash.exe"), "wsl launcher").unwrap();
        let env = vec![("PATH".to_string(), wsl_dir.display().to_string())];

        assert!(matches!(
            ensure_bash_installed_with_env(&env),
            Err(ScriptError::DependencyMissing { name, .. }) if name == "bash"
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_ensure_git_installed() {
        assert!(ensure_git_installed().is_ok());
    }

    #[test]
    fn test_ensure_command_check_failed_with_non_empty_stderr() {
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/C", "echo boom 1>&2 & exit /B 1"]);
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c", "printf boom >&2; exit 1"]);

        let result = ensure_command(program, &args, "hint");
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::DependencyCheckFailed { name, message } => {
                assert_eq!(name, program);
                assert!(message.contains("boom"));
            }
            other => panic!("expected DependencyCheckFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_ensure_powershell_installed_returns_result() {
        // Only assert that the call returns a Result; pwsh may or may not
        // be installed in the dev environment. The point is to exercise
        // the wrapper code path.
        let _ = ensure_powershell_installed();
    }

    #[test]
    fn test_ensure_python_installed_returns_result() {
        let _ = ensure_python_installed();
    }

    #[test]
    fn test_ensure_jq_installed_returns_result() {
        let _ = ensure_jq_installed();
    }
    #[cfg(unix)]
    #[test]
    fn test_dependency_checks_use_injected_path_for_every_runtime() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        for program in ["git", "jq", "bash", python_program(), powershell_program()] {
            let path = dir.path().join(program);
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        let env = vec![("PATH".to_string(), dir.path().display().to_string())];

        assert!(ensure_git_installed_with_env(&env).is_ok());
        assert!(ensure_jq_installed_with_env(&env).is_ok());
        assert!(ensure_bash_installed_with_env(&env).is_ok());
        assert!(ensure_python_installed_with_env(&env).is_ok());
        assert!(ensure_powershell_installed_with_env(&env).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_dependency_checks_reject_missing_injected_path_program() {
        let dir = tempfile::tempdir().unwrap();
        let env = vec![("PATH".to_string(), dir.path().display().to_string())];

        let result = ensure_jq_installed_with_env(&env);
        assert!(matches!(
            result,
            Err(ScriptError::DependencyMissing { name, .. }) if name == "jq"
        ));
    }
}
