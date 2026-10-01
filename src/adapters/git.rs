use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub(crate) struct GitProcess<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub allowed_protocols: &'a str,
    pub global_config: Option<&'a Path>,
    pub askpass: Option<&'a Path>,
    pub credential_authority: Option<&'a str>,
    pub curlopt_resolve: Option<&'a str>,
}

pub(crate) enum GitProbeError {
    Spawn(io::Error),
    Wait(io::Error),
    Timeout(Duration),
}

pub(crate) struct GitProbeOutput {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: Vec<u8>,
}

pub(crate) fn run(process: &GitProcess<'_>) -> io::Result<Output> {
    command(process).output()
}

pub(crate) fn run_with_timeout(
    process: &GitProcess<'_>,
    timeout: Duration,
) -> Result<GitProbeOutput, GitProbeError> {
    let mut child = command(process)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(GitProbeError::Spawn)?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                let mut stderr = Vec::new();
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr);
                }
                return Ok(GitProbeOutput {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GitProbeError::Timeout(timeout));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => return Err(GitProbeError::Wait(err)),
        }
    }
}

pub(crate) fn command(process: &GitProcess<'_>) -> Command {
    let mut command = Command::new(process.program);
    command
        .args(["-c", "http.followRedirects=false"])
        .args(["-c", "http.proxy="])
        .args(["-c", "core.autocrlf=false"]);
    #[cfg(windows)]
    command.args(["-c", "core.filemode=false"]);
    if let Some(resolve) = process.curlopt_resolve {
        command.args(["-c", &format!("http.curloptResolve={resolve}")]);
    }
    command
        .args(process.args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ALLOW_PROTOCOL", process.allowed_protocols);
    if let Some(path) = process.global_config {
        command.env("GIT_CONFIG_GLOBAL", path);
    } else {
        command.env_remove("GIT_CONFIG_GLOBAL");
    }
    command
        .env_remove("SSH_ASKPASS")
        .env_remove("GIT_SSH")
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_TEMPLATE_DIR")
        .env_remove("GIT_EXEC_PATH")
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_CONFIG_DIRS")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("OMAKURE_API_TOKEN")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env_remove("no_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("NO_PROXY");
    if let Some(askpass) = process.askpass {
        command
            .env("GIT_ASKPASS", askpass)
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(authority) = process.credential_authority {
            command.env("OMAKURE_GIT_AUTHORITY", authority);
        } else {
            command.env_remove("OMAKURE_GIT_AUTHORITY");
        }
    } else {
        command
            .env_remove("GIT_ASKPASS")
            .env_remove("OMAKURE_GIT_AUTHORITY");
    }
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn timed_probe_kills_child_and_reports_timeout() {
        let dir = crate::util::exec::generated_executable_tempdir().unwrap();
        let shim = dir.path().join("git-probe");
        crate::util::exec::write_generated_executable(&shim, b"#!/bin/sh\nexec sleep 5\n").unwrap();
        let process = GitProcess {
            program: shim.to_str().unwrap(),
            args: &[],
            allowed_protocols: "file:https:http",
            global_config: None,
            askpass: None,
            credential_authority: None,
            curlopt_resolve: None,
        };
        let start = Instant::now();
        let result = run_with_timeout(&process, Duration::from_millis(50));
        assert!(matches!(result, Err(GitProbeError::Timeout(_))));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
