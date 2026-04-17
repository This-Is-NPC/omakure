use std::process::Command;

use crate::error::ScriptError;
use crate::runtime::{powershell_program, python_program};

/// Check that a command is available and runs successfully.
fn ensure_command(program: &str, args: &[&str], not_found_hint: &str) -> Result<(), ScriptError> {
    match Command::new(program).args(args).output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim().to_string();
                Err(ScriptError::DependencyCheckFailed {
                    name: program.to_string(),
                    message: if message.is_empty() {
                        "check failed".to_string()
                    } else {
                        message
                    },
                })
            }
        }
        Err(_) => Err(ScriptError::DependencyMissing {
            name: program.to_string(),
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

#[cfg(windows)]
pub(crate) fn ensure_bash_installed() -> Result<(), ScriptError> {
    ensure_command(
        "bash",
        &["--version"],
        "Install Git for Windows or add bash.exe to PATH",
    )
}

#[cfg(not(windows))]
pub(crate) fn ensure_bash_installed() -> Result<(), ScriptError> {
    ensure_command(
        "bash",
        &["--version"],
        "Install bash and ensure it is in PATH",
    )
}

pub(crate) fn ensure_jq_installed() -> Result<(), ScriptError> {
    ensure_command("jq", &["--version"], "Install jq and ensure it is in PATH")
}

pub(crate) fn ensure_powershell_installed() -> Result<(), ScriptError> {
    let program = powershell_program();
    ensure_command(
        program,
        &["-NoProfile", "-Command", "$PSVersionTable.PSVersion"],
        &format!("Install PowerShell and ensure {} is in PATH", program),
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
        let result = ensure_command("false", &[], "should fail");
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::DependencyCheckFailed { name, .. } => {
                assert_eq!(name, "false");
            }
            other => panic!("expected DependencyCheckFailed, got {:?}", other),
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn test_ensure_bash_installed() {
        assert!(ensure_bash_installed().is_ok());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_ensure_git_installed() {
        assert!(ensure_git_installed().is_ok());
    }

    #[test]
    fn test_ensure_command_check_failed_with_non_empty_stderr() {
        // `bash -c 'echo boom 1>&2; exit 1'` exits non-zero with stderr.
        let result = ensure_command("bash", &["-c", "echo boom 1>&2; exit 1"], "hint");
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::DependencyCheckFailed { name, message } => {
                assert_eq!(name, "bash");
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
}
