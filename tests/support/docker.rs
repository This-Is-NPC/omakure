use super::compose_env::ComposeEnv;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const COMPOSE_OPERATION_TIMEOUT: &str = "120s";
const COMPOSE_BUILD_TIMEOUT: &str = "1800s";

fn bounded_command_within(program: &str, budget: &str) -> Command {
    let mut command = Command::new("timeout");
    command.args(["--foreground", "--kill-after=10s", budget, program]);
    command
}

pub fn bounded_command(program: &str) -> Command {
    bounded_command_within(program, COMPOSE_OPERATION_TIMEOUT)
}

fn compose_timeout(args: &[&str]) -> &'static str {
    if args.contains(&"--build") {
        COMPOSE_BUILD_TIMEOUT
    } else {
        COMPOSE_OPERATION_TIMEOUT
    }
}

pub fn compose_command(root: &Path, env: &ComposeEnv, prefix: &[&str], args: &[&str]) -> Command {
    let mut command = bounded_command_within("docker", compose_timeout(args));
    env.apply(&mut command);
    command
        .current_dir(root)
        .arg("compose")
        .args(prefix)
        .args(args);
    command
}

pub fn wait_until(timeout: Duration, interval: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(interval);
    }
    false
}

pub fn cleanup_project(root: &Path, env: &ComposeEnv, project: &str) -> Result<(), String> {
    let mut failures = Vec::new();
    let down = compose_command(
        root,
        env,
        &["-p", project],
        &["down", "--volumes", "--remove-orphans"],
    )
    .output()
    .expect("run docker compose");
    if !down.status.success() {
        failures.push(format!(
            "compose down status={} stderr={}",
            down.status,
            safe_stderr(&down)
        ));
    }
    for resource in ["container", "network", "volume"] {
        let output = bounded_command("docker")
            .args([
                resource,
                "ls",
                "-q",
                "--filter",
                &format!("label=com.docker.compose.project={project}"),
            ])
            .output();
        match output {
            Ok(output) if !output.status.success() => failures.push(format!(
                "inspect {resource} status={} stderr={}",
                output.status,
                safe_stderr(&output)
            )),
            Ok(output) if !String::from_utf8_lossy(&output.stdout).trim().is_empty() => {
                failures.push(format!("project-labeled {resource} remains"));
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("inspect {resource}: {error}")),
        }
    }
    cleanup_result(failures)
}

fn cleanup_result(failures: Vec<String>) -> Result<(), String> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn safe_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

pub fn safe_generation_stderr(output: &Output) -> String {
    let stderr = safe_stderr(output);
    let lower = stderr.to_ascii_lowercase();
    assert!(
        !lower.contains("bearer ") && !lower.contains("$argon2") && !lower.contains("token ="),
        "token generation stderr contained sensitive material"
    );
    stderr
}

pub fn output_text(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn json_output(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "expected successful command: {}",
        output_text(output)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON output ({error}): {}", output_text(output)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_reports_all_failures() {
        let error = cleanup_result(vec!["down failed".into(), "volume remains".into()])
            .expect_err("cleanup failure should be returned");
        assert!(error.contains("down failed"));
        assert!(error.contains("volume remains"));
    }

    #[test]
    fn compose_command_keeps_build_budget_and_child_only_environment() {
        let env = ComposeEnv::new(
            "target.tokens".into(),
            "target.client".into(),
            "candidate.tokens".into(),
            "candidate.client".into(),
        );
        let command = compose_command(
            Path::new("/workspace"),
            &env,
            &["-p", "project"],
            &["up", "--build", "-d"],
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert_eq!(
            args,
            [
                "--foreground",
                "--kill-after=10s",
                "1800s",
                "docker",
                "compose",
                "-p",
                "project",
                "up",
                "--build",
                "-d"
            ]
        );
        assert!(command
            .get_envs()
            .any(|(key, _)| key == "OMAKURE_ENROLLMENT_TARGET_TOKENS_FILE"));
    }
}
