use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::ScriptError;
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScriptKind {
    Bash,
    PowerShell,
    Python,
}

pub fn script_kind(path: &Path) -> Option<ScriptKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "bash" | "sh" => Some(ScriptKind::Bash),
        "ps1" => Some(ScriptKind::PowerShell),
        "py" => Some(ScriptKind::Python),
        _ => None,
    }
}

pub fn script_extensions() -> &'static [&'static str] {
    &["bash", "sh", "ps1", "py"]
}

pub fn command_for_script(script: &Path) -> Result<Command, ScriptError> {
    command_for_script_with_env(script, &[])
}

/// Like [`command_for_script`], but honors an injected environment when
/// choosing the interpreter binary.
///
/// Per the locked spike decision (`tests/spike_command_path_resolution.rs`,
/// task 1751): if `env` carries a `PATH` entry (e.g. a venv-prepended PATH
/// produced by env injection), the interpreter name is resolved to an
/// ABSOLUTE path via a which-style lookup against that injected PATH, then
/// spawned as `Command::new(abs_path)`. This removes the silent
/// wrong-interpreter footgun: relying on `Command::new("python3")` name
/// resolution honoring the child `PATH` is a std implementation detail that
/// differs across platforms.
///
/// The mechanism is language-agnostic — it merely resolves the interpreter
/// program against the (possibly venv-prepended) PATH — so the same code path
/// serves python `.venv`, node `nvm`/`node_modules/.bin`, ruby `rbenv`, etc.,
/// with no per-language configuration.
///
/// FALLBACK (no regression): when `env` has no `PATH` entry, or the
/// interpreter is not found on the injected PATH, the original name-based
/// behavior (`Command::new("python3")`) is preserved. The parent process
/// PATH is intentionally NOT scanned in the fallback — the child inherits it
/// and std resolves the name against it as before.
pub fn command_for_script_with_env(
    script: &Path,
    env: &[(String, String)],
) -> Result<Command, ScriptError> {
    let kind = script_kind(script).ok_or(ScriptError::UnsupportedType)?;
    let program: &str = match kind {
        ScriptKind::Bash => "bash",
        ScriptKind::PowerShell => powershell_program(),
        ScriptKind::Python => python_program(),
    };

    // Resolve to an absolute path only when an injected PATH is present and
    // actually contains the interpreter; otherwise keep name-based behavior.
    let mut command = match resolve_interpreter(program, env) {
        Some(abs_path) => Command::new(abs_path),
        None => Command::new(program),
    };

    match kind {
        ScriptKind::Bash | ScriptKind::Python => {
            command.arg(script);
        }
        ScriptKind::PowerShell => {
            command.arg("-NoProfile").arg("-File").arg(script);
        }
    }

    Ok(command)
}

/// Return the injected `PATH` value from `env`, then resolve `program` against
/// it. An exact `PATH` key is preferred because that is what Unix exec lookup
/// uses; if absent, fall back to a case-insensitive match so Windows-style
/// `Path` remains useful. Within each key class, last write wins, matching
/// `cmd.env` semantics.
pub(crate) fn resolve_interpreter(program: &str, env: &[(String, String)]) -> Option<PathBuf> {
    let exact_path = env
        .iter()
        .rev()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_str());
    let injected_path = exact_path.or_else(|| {
        env.iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v.as_str())
    })?;
    resolve_program_in_path(program, injected_path)
}

/// Which-style lookup: resolve a bare program name to the first executable
/// file found by scanning the platform-delimited `path_var` left-to-right.
///
/// - Only bare names are resolved; a `program` that already contains a path
///   separator is returned as `None` (the OS handles it verbatim).
/// - Only absolute directory entries are considered, so the returned path is
///   always absolute and independent of the current working directory.
/// - On Unix a candidate must be a regular file with an executable bit set;
///   on other platforms it must simply be a file.
pub fn resolve_program_in_path(program: &str, path_var: &str) -> Option<PathBuf> {
    if program.contains('/') || program.contains(std::path::MAIN_SEPARATOR) {
        return None;
    }
    for dir in std::env::split_paths(path_var) {
        if !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

pub fn powershell_program() -> &'static str {
    if cfg!(windows) {
        "powershell"
    } else {
        "pwsh"
    }
}

pub fn python_program() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::Path;

    #[rstest]
    #[case::sh_extension("script.sh", Some(ScriptKind::Bash))]
    #[case::bash_extension("script.bash", Some(ScriptKind::Bash))]
    #[case::uppercase_sh("script.SH", Some(ScriptKind::Bash))]
    #[case::powershell("script.ps1", Some(ScriptKind::PowerShell))]
    #[case::uppercase_ps1("script.PS1", Some(ScriptKind::PowerShell))]
    #[case::python("script.py", Some(ScriptKind::Python))]
    #[case::uppercase_py("script.PY", Some(ScriptKind::Python))]
    #[case::unknown_extension("script.txt", None)]
    #[case::rust_extension("script.rs", None)]
    #[case::no_extension("no_ext", None)]
    fn test_script_kind(#[case] path: &str, #[case] expected: Option<ScriptKind>) {
        assert_eq!(script_kind(Path::new(path)), expected);
    }

    #[test]
    fn test_script_extensions_contains_all_supported() {
        let exts = script_extensions();
        assert!(exts.contains(&"bash"));
        assert!(exts.contains(&"sh"));
        assert!(exts.contains(&"ps1"));
        assert!(exts.contains(&"py"));
        assert_eq!(exts.len(), 4);
    }

    #[rstest]
    #[case::bash_command_for_sh("script.sh", "bash")]
    #[case::bash_command_for_bash("script.bash", "bash")]
    #[case::python_command("script.py", python_program())]
    #[case::powershell_command("script.ps1", powershell_program())]
    fn test_command_for_script_program(#[case] path: &str, #[case] expected_program: &str) {
        let cmd = command_for_script(Path::new(path)).unwrap();
        assert_eq!(cmd.get_program(), expected_program);
    }

    #[test]
    fn test_command_for_script_unsupported() {
        let result = command_for_script(Path::new("script.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_command_for_script_ps1_has_noprofile_flag() {
        let cmd = command_for_script(Path::new("script.ps1")).unwrap();
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args.contains(&std::ffi::OsStr::new("-NoProfile")));
        assert!(args.contains(&std::ffi::OsStr::new("-File")));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_powershell_program_is_pwsh_on_unix() {
        assert_eq!(powershell_program(), "pwsh");
    }

    #[cfg(not(windows))]
    #[test]
    fn test_python_program_is_python3_on_unix() {
        assert_eq!(python_program(), "python3");
    }

    // --- Task 1755: absolute-path interpreter resolution against injected PATH ---

    #[cfg(unix)]
    const SHIM_MARKER: &str = "OMAKURE_RUNTIME_SHIM_MARKER";

    /// Write an executable shim named `name` into `dir` that prints
    /// [`SHIM_MARKER`] and ignores its arguments, so the marker in stdout is
    /// unambiguous proof the shim (not the system interpreter) executed.
    #[cfg(unix)]
    fn write_shim(dir: &Path, name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let shim = dir.join(name);
        std::fs::write(&shim, format!("#!/bin/sh\necho {SHIM_MARKER}\n")).unwrap();
        let mut perms = std::fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim, perms).unwrap();
        shim
    }

    #[cfg(unix)]
    #[test]
    fn resolve_program_in_path_finds_first_executable() {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(dir.path(), "python3");
        let path_var = format!("{}:/nonexistent-dir-xyz", dir.path().display());

        let resolved = resolve_program_in_path("python3", &path_var).expect("shim must be found");
        assert_eq!(resolved, shim);
        assert!(resolved.is_absolute(), "resolved path must be absolute");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_program_in_path_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap(); // empty dir, no python3
        let path_var = dir.path().display().to_string();
        assert!(resolve_program_in_path("python3", &path_var).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_program_in_path_ignores_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        // A plain (non-executable) file named python3 must be skipped.
        std::fs::write(dir.path().join("python3"), "not executable").unwrap();
        let path_var = dir.path().display().to_string();
        assert!(resolve_program_in_path("python3", &path_var).is_none());
    }

    /// Headline proof: with an injected PATH prepending a shim `python3`, the
    /// built command targets the ABSOLUTE shim path and executing it actually
    /// runs the shim (marker in stdout) — not the system interpreter.
    #[cfg(unix)]
    #[test]
    fn command_for_script_with_env_resolves_and_runs_injected_shim() {
        let shim_dir = tempfile::tempdir().unwrap();
        let shim = write_shim(shim_dir.path(), "python3");

        let script_dir = tempfile::tempdir().unwrap();
        let script = script_dir.path().join("job.py");
        std::fs::write(&script, "print('would-be-system-python')").unwrap();

        let inherited = std::env::var("PATH").unwrap_or_default();
        let injected = format!("{}:{}", shim_dir.path().display(), inherited);
        let env = vec![("PATH".to_string(), injected)];

        let mut cmd = command_for_script_with_env(&script, &env).unwrap();
        assert_eq!(
            cmd.get_program(),
            shim.as_os_str(),
            "interpreter must resolve to the absolute shim path"
        );

        let out = cmd.output().expect("spawn resolved shim");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(SHIM_MARKER),
            "the shim (not system python) must run, got {stdout:?}"
        );
    }

    /// Fallback: no injected PATH -> name-based behavior is unchanged (no
    /// regression). The program stays the literal `python_program()`.
    #[cfg(unix)]
    #[test]
    fn command_for_script_with_env_falls_back_without_injected_path() {
        let script_dir = tempfile::tempdir().unwrap();
        let script = script_dir.path().join("job.py");
        std::fs::write(&script, "print('hi')").unwrap();

        let cmd = command_for_script_with_env(&script, &[]).unwrap();
        assert_eq!(cmd.get_program(), python_program());
    }

    /// Edge: injected PATH points at a dir with no interpreter -> graceful
    /// fallback to name-based behavior (documented).
    #[cfg(unix)]
    #[test]
    fn command_for_script_with_env_falls_back_when_interpreter_absent() {
        let empty = tempfile::tempdir().unwrap();
        let script_dir = tempfile::tempdir().unwrap();
        let script = script_dir.path().join("job.py");
        std::fs::write(&script, "print('hi')").unwrap();

        let env = vec![("PATH".to_string(), empty.path().display().to_string())];
        let cmd = command_for_script_with_env(&script, &env).unwrap();
        assert_eq!(cmd.get_program(), python_program());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_interpreter_prefers_exact_path_over_case_variant() {
        let exact_dir = tempfile::tempdir().unwrap();
        let exact_shim = write_shim(exact_dir.path(), "python3");
        let variant_dir = tempfile::tempdir().unwrap();
        let _variant_shim = write_shim(variant_dir.path(), "python3");
        let env = vec![
            ("Path".to_string(), variant_dir.path().display().to_string()),
            ("PATH".to_string(), exact_dir.path().display().to_string()),
        ];

        let resolved = resolve_interpreter("python3", &env).expect("exact PATH shim found");
        assert_eq!(resolved, exact_shim);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_interpreter_falls_back_to_case_insensitive_path() {
        let variant_dir = tempfile::tempdir().unwrap();
        let variant_shim = write_shim(variant_dir.path(), "python3");
        let env = vec![("Path".to_string(), variant_dir.path().display().to_string())];

        let resolved = resolve_interpreter("python3", &env).expect("Path shim found");
        assert_eq!(resolved, variant_shim);
    }
}
