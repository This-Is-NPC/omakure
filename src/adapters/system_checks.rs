use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
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

const MAX_INJECTED_PATH_BYTES: usize = 32 * 1024;

/// Build a minimal PATH for spawning an already-resolved absolute executable.
fn bounded_spawn_path(program: &Path) -> OsString {
    let mut paths = Vec::new();
    if let Some(parent) = program.parent() {
        if !parent.as_os_str().is_empty() {
            paths.push(parent.to_path_buf());
        }
    }

    #[cfg(windows)]
    {
        let system_root =
            std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from("C:\\Windows"));
        paths.push(PathBuf::from(system_root).join("System32"));
    }

    #[cfg(all(unix, target_os = "macos"))]
    {
        paths.push(PathBuf::from("/usr/bin"));
        paths.push(PathBuf::from("/bin"));
        paths.push(PathBuf::from("/usr/sbin"));
        paths.push(PathBuf::from("/sbin"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        paths.push(PathBuf::from("/usr/bin"));
        paths.push(PathBuf::from("/bin"));
    }

    std::env::join_paths(paths).expect("bounded PATH components are valid")
}

fn child_path_for_absolute_spawn(program: &Path, env: &[(String, String)]) -> Option<OsString> {
    let injected = crate::runtime::path_value(env)?;
    if injected.len() <= MAX_INJECTED_PATH_BYTES {
        Some(OsString::from(injected))
    } else {
        Some(bounded_spawn_path(program))
    }
}

/// Check a command using an already-resolved executable and injected env.
fn ensure_command_os_with_env(
    program: impl AsRef<OsStr>,
    name: &str,
    args: &[&str],
    not_found_hint: &str,
    env: &[(String, String)],
) -> Result<(), ScriptError> {
    let program = program.as_ref();
    let mut command = Command::new(program);
    if Path::new(program).is_absolute() {
        for (key, value) in env {
            if !key.eq_ignore_ascii_case("PATH") {
                command.env(key, value);
            }
        }
        if let Some(child_path) = child_path_for_absolute_spawn(Path::new(program), env) {
            command.env("PATH", child_path);
        }
    } else {
        command.envs(env.iter().map(|(key, value)| (key, value)));
    }
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
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(ScriptError::DependencyMissing {
            name: name.to_string(),
            hint: not_found_hint.to_string(),
        }),
        Err(err) => Err(ScriptError::DependencyCheckFailed {
            name: name.to_string(),
            message: err.to_string(),
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

#[cfg(all(test, unix))]
pub(crate) fn write_test_executable_shim(dir: &Path, program: &str) {
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::thread;
    use std::time::Duration;

    const ETXTBSY: i32 = 26;
    const MAX_PROBE_ATTEMPTS: u32 = 8;
    const PROBE_DELAY: Duration = Duration::from_millis(5);

    let path = dir.join(program);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&path)
        .unwrap_or_else(|error| panic!("open {program} fixture: {error}"));
    file.write_all(b"#!/bin/sh\nexit 0\n")
        .unwrap_or_else(|error| panic!("write {program} fixture: {error}"));
    file.sync_all()
        .unwrap_or_else(|error| panic!("sync {program} fixture: {error}"));
    drop(file);
    if let Ok(dir_file) = File::open(dir) {
        let _ = dir_file.sync_all();
    }

    for attempt in 1..=MAX_PROBE_ATTEMPTS {
        match Command::new(&path).status() {
            Ok(status) if status.success() => return,
            Ok(status) => panic!(
                "probe exec {} fixture: exited with {status}",
                path.display()
            ),
            Err(err) if err.raw_os_error() == Some(ETXTBSY) => {
                if attempt == MAX_PROBE_ATTEMPTS {
                    panic!(
                        "probe exec {} fixture still ETXTBSY after {MAX_PROBE_ATTEMPTS} attempts: {err}",
                        path.display()
                    );
                }
                thread::sleep(PROBE_DELAY);
            }
            Err(err) => panic!("probe exec {} fixture: {err}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type DependencyCheck = fn(&[(String, String)]) -> Result<(), ScriptError>;

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
        let dir = tempfile::tempdir().unwrap();
        let programs = ["git", "jq", "bash", python_program(), powershell_program()];
        for program in programs {
            write_test_executable_shim(dir.path(), program);
        }
        let env = vec![("PATH".to_string(), dir.path().display().to_string())];

        let checks: [(&str, DependencyCheck); 5] = [
            ("git", ensure_git_installed_with_env),
            ("jq", ensure_jq_installed_with_env),
            ("bash", ensure_bash_installed_with_env),
            (python_program(), ensure_python_installed_with_env),
            (powershell_program(), ensure_powershell_installed_with_env),
        ];
        for (program, check) in checks {
            let result = check(&env);
            assert!(
                result.is_ok(),
                "{program} dependency check failed using its injected fixture: {result:?}"
            );
        }
    }

    #[cfg(unix)]
    fn huge_injected_path_with_shim_dir(shim_dir: &Path) -> String {
        let mut path = shim_dir.display().to_string();
        let mut index = 0usize;
        while path.len() <= MAX_INJECTED_PATH_BYTES {
            path.push_str(&format!(":/tmp/omakure-path-padding-{index}"));
            index += 1;
        }
        path
    }

    #[cfg(unix)]
    #[test]
    fn test_dependency_checks_succeed_with_huge_injected_path() {
        let dir = tempfile::tempdir().unwrap();
        write_test_executable_shim(dir.path(), "git");
        write_test_executable_shim(dir.path(), "jq");

        let env = vec![(
            "PATH".to_string(),
            huge_injected_path_with_shim_dir(dir.path()),
        )];
        assert!(
            env[0].1.len() > MAX_INJECTED_PATH_BYTES,
            "fixture PATH must exceed the bounded-spawn threshold"
        );

        assert!(
            ensure_git_installed_with_env(&env).is_ok(),
            "git check should resolve via huge PATH and spawn via bounded child PATH"
        );
        assert!(
            ensure_jq_installed_with_env(&env).is_ok(),
            "jq check should resolve via huge PATH and spawn via bounded child PATH"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_spawn_of_file_open_for_write_is_etxtbsy() {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        fn assert_etxtbsy(result: Result<(), ScriptError>) {
            assert!(result.is_err());
            match result.unwrap_err() {
                ScriptError::DependencyCheckFailed { name, message } => {
                    assert_eq!(name, "git");
                    assert!(
                        message.contains("Text file busy") || message.contains("os error 26"),
                        "expected ETXTBSY, got: {message}"
                    );
                }
                other => panic!("expected DependencyCheckFailed, got {:?}", other),
            }
        }

        fn message_is_etxtbsy(message: &str) -> bool {
            message.contains("Text file busy") || message.contains("os error 26")
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("git");

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&path)
            .unwrap();
        file.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        file.sync_all().unwrap();

        let shebang_result = ensure_command_os_with_env(&path, "git", &["--version"], "hint", &[]);
        if matches!(
            &shebang_result,
            Err(ScriptError::DependencyCheckFailed { message, .. })
                if message_is_etxtbsy(message)
        ) {
            assert_etxtbsy(shebang_result);
            return;
        }

        // Shebang exec may not bump the kernel ELF i_writecount on every kernel;
        // copy a real ELF binary and keep a writable fd open to force ETXTBSY.
        drop(file);
        let true_src = ["/usr/bin/true", "/bin/true"]
            .into_iter()
            .find(|candidate| Path::new(candidate).exists())
            .expect("need /usr/bin/true or /bin/true for ELF ETXTBSY fixture");
        std::fs::copy(true_src, &path).unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();

        let elf_result = ensure_command_os_with_env(&path, "git", &["--version"], "hint", &[]);
        assert_etxtbsy(elf_result);
        drop(file);
    }

    #[cfg(unix)]
    #[test]
    fn test_ensure_command_os_spawn_error_is_check_failed_not_missing() {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-executable");
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o644)
                .open(&path)
                .unwrap();
            file.write_all(b"not a script\n").unwrap();
        }

        let result = ensure_command_os_with_env(&path, "probe", &["--version"], "hint", &[]);
        assert!(result.is_err());
        match result.unwrap_err() {
            ScriptError::DependencyCheckFailed { name, message } => {
                assert_eq!(name, "probe");
                assert!(
                    !message.is_empty(),
                    "spawn failure should surface the underlying io error"
                );
            }
            ScriptError::DependencyMissing { .. } => {
                panic!("expected DependencyCheckFailed for non-NotFound spawn error");
            }
            other => panic!("unexpected error: {other:?}"),
        }
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
