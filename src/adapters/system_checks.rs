use std::error::Error;
use std::process::Command;

use crate::runtime::{powershell_program, python_program};

#[cfg(windows)]
pub(crate) fn ensure_git_installed() -> Result<(), Box<dyn Error>> {
    match Command::new("git").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim();
                if message.is_empty() {
                    Err("Git found, but `git --version` failed".into())
                } else {
                    Err(format!("Git found, but `git --version` failed: {}", message).into())
                }
            }
        }
        Err(err) => Err(format!(
            "Git not found in PATH. Install Git for Windows (includes bash): {}",
            err
        )
        .into()),
    }
}

#[cfg(not(windows))]
pub(crate) fn ensure_git_installed() -> Result<(), Box<dyn Error>> {
    match Command::new("git").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim();
                if message.is_empty() {
                    Err("Git found, but `git --version` failed".into())
                } else {
                    Err(format!("Git found, but `git --version` failed: {}", message).into())
                }
            }
        }
        Err(err) => Err(format!(
            "Git not found in PATH. Install Git and ensure it is in PATH: {}",
            err
        )
        .into()),
    }
}

#[cfg(windows)]
pub(crate) fn ensure_bash_installed() -> Result<(), Box<dyn Error>> {
    match Command::new("bash").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim();
                if message.is_empty() {
                    Err("Bash found, but `bash --version` failed".into())
                } else {
                    Err(format!("Bash found, but `bash --version` failed: {}", message).into())
                }
            }
        }
        Err(err) => Err(format!(
            "Bash not found in PATH. Install Git for Windows or add bash.exe to PATH: {}",
            err
        )
        .into()),
    }
}

#[cfg(not(windows))]
pub(crate) fn ensure_bash_installed() -> Result<(), Box<dyn Error>> {
    match Command::new("bash").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim();
                if message.is_empty() {
                    Err("Bash found, but `bash --version` failed".into())
                } else {
                    Err(format!("Bash found, but `bash --version` failed: {}", message).into())
                }
            }
        }
        Err(err) => Err(format!(
            "Bash not found in PATH. Install bash and ensure it is in PATH: {}",
            err
        )
        .into()),
    }
}

pub(crate) fn ensure_jq_installed() -> Result<(), Box<dyn Error>> {
    match Command::new("jq").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim();
                if message.is_empty() {
                    Err("jq found, but `jq --version` failed".into())
                } else {
                    Err(format!("jq found, but `jq --version` failed: {}", message).into())
                }
            }
        }
        Err(err) => Err(format!(
            "jq not found in PATH. Install jq and ensure it is in PATH: {}",
            err
        )
        .into()),
    }
}

pub(crate) fn ensure_powershell_installed() -> Result<(), Box<dyn Error>> {
    let program = powershell_program();
    match Command::new(program)
        .args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion"])
        .output()
    {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim();
                if message.is_empty() {
                    Err(format!("{} found, but PowerShell check failed", program).into())
                } else {
                    Err(format!(
                        "{} found, but PowerShell check failed: {}",
                        program, message
                    )
                    .into())
                }
            }
        }
        Err(err) => Err(format!(
            "{} not found in PATH. Install PowerShell and ensure it is in PATH: {}",
            program, err
        )
        .into()),
    }
}

pub(crate) fn ensure_python_installed() -> Result<(), Box<dyn Error>> {
    let program = python_program();
    match Command::new(program).arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let message = stderr.trim();
                if message.is_empty() {
                    Err(format!("{} found, but `--version` failed", program).into())
                } else {
                    Err(format!(
                        "{} found, but `--version` failed: {}",
                        program, message
                    )
                    .into())
                }
            }
        }
        Err(err) => Err(format!(
            "{} not found in PATH. Install Python and ensure it is in PATH: {}",
            program, err
        )
        .into()),
    }
}
