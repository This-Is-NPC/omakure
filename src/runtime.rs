use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::ScriptError;
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ScriptKind {
    Bash,
    PowerShell,
    Python,
    /// Executed by a Lua runtime embedded in this binary; no host interpreter.
    Lua,
}

pub fn script_kind(path: &Path) -> Option<ScriptKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "bash" | "sh" => Some(ScriptKind::Bash),
        "ps1" => Some(ScriptKind::PowerShell),
        "py" => Some(ScriptKind::Python),
        "lua" => Some(ScriptKind::Lua),
        _ => None,
    }
}

/// Supported script extensions, in resolution precedence order.
///
/// This doubles as the precedence list for extensionless lookups
/// (`operations::core`, `cli::run`), so `lua` is appended last: `omakure run
/// deploy` keeps resolving `deploy.sh` when both it and `deploy.lua` exist.
pub fn script_extensions() -> &'static [&'static str] {
    &["bash", "sh", "ps1", "py", "lua"]
}

/// The argv marker that puts this binary into embedded-Lua host mode.
///
/// Intercepted in `main` before clap runs. It is not a subcommand: `--json` and
/// `--scripts-dir` are declared global, so a subcommand would let omakure's own
/// parser consume a script's arguments, and `--help` would print omakure's help
/// and exit 0.
pub const LUA_HOST_ARG: &str = "--__omakure-lua-host";

/// Exit code for a failure of the Lua *host*, as opposed to the script.
///
/// A script's own `os.exit(1)` must stay distinguishable from "the host could
/// not start". 126 follows the shell convention for "found but not executable".
pub const LUA_HOST_FAILURE_EXIT: i32 = 126;

/// Resolve the binary that will host embedded Lua.
///
/// `current_exe()` only. No environment override and no `PATH` fallback: this
/// is a spawn path inside a process that runs as a service, and either would be
/// an arbitrary-binary-execution vector. Note that `cli::update` replaces the
/// binary at this path via rename, so a worker that survives a self-update will
/// exec the replaced file; the error below is what makes that legible.
fn lua_host_binary() -> Result<PathBuf, ScriptError> {
    let exe = std::env::current_exe().map_err(|err| ScriptError::HostBinaryUnavailable {
        reason: err.to_string(),
    })?;
    if !exe.is_file() {
        return Err(ScriptError::HostBinaryUnavailable {
            reason: format!("{} is not a file", exe.display()),
        });
    }
    Ok(exe)
}

/// Build a command while honoring an injected environment when choosing the
/// interpreter binary.
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
    match kind {
        ScriptKind::Bash => bash_script_command(script, env),
        ScriptKind::PowerShell => powershell_script_command(script, env),
        ScriptKind::Python => python_script_command(script, env),
        ScriptKind::Lua => lua_script_command(script),
    }
}

fn bash_script_command(script: &Path, env: &[(String, String)]) -> Result<Command, ScriptError> {
    let mut command = bash_command_with_env(env)?;
    command.arg(script);
    Ok(command)
}

fn powershell_script_command(
    script: &Path,
    env: &[(String, String)],
) -> Result<Command, ScriptError> {
    let mut command = command_for_interpreter(powershell_program(), env);
    command.args(["-NoProfile", "-File"]).arg(script);
    Ok(command)
}

fn python_script_command(script: &Path, env: &[(String, String)]) -> Result<Command, ScriptError> {
    let mut command = command_for_interpreter(python_program(), env);
    command.arg(script);
    Ok(command)
}

fn lua_script_command(script: &Path) -> Result<Command, ScriptError> {
    let mut command = Command::new(lua_host_binary()?);
    command.arg(LUA_HOST_ARG).arg(script);
    Ok(command)
}

fn command_for_interpreter(program: &str, env: &[(String, String)]) -> Command {
    match resolve_interpreter(program, env) {
        Some(abs_path) => Command::new(abs_path),
        None => Command::new(program),
    }
}

/// The hint used when a Windows installation has no native Git Bash.
#[cfg(windows)]
pub(crate) const BASH_MISSING_HINT: &str =
    "Install Git for Windows (Git Bash) and ensure bash.exe is in PATH";

pub(crate) fn path_value(env: &[(String, String)]) -> Option<&str> {
    let exact_path = env
        .iter()
        .rev()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_str());
    exact_path.or_else(|| {
        env.iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v.as_str())
    })
}

/// Resolve the native Bash executable used for Windows script execution.
///
/// Windows commonly exposes `C:\Windows\System32\bash.exe`, but that binary
/// is the WSL launcher rather than Git Bash. It is deliberately skipped so
/// dependency checks and script execution cannot disagree about the runtime.
pub(crate) fn resolve_bash_program(env: &[(String, String)]) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let path = path_value(env)
            .map(str::to_owned)
            .or_else(|| std::env::var("PATH").ok())?;
        return resolve_program_in_path("bash", &path).filter(|path| !is_wsl_launcher(path));
    }
    #[cfg(not(windows))]
    {
        resolve_interpreter("bash", env)
    }
}

#[cfg(windows)]
fn is_wsl_launcher(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    normalized.ends_with("\\windows\\system32\\bash.exe")
        || normalized.ends_with("\\windows\\sysnative\\bash.exe")
}

fn bash_command_with_env(env: &[(String, String)]) -> Result<Command, ScriptError> {
    #[cfg(windows)]
    {
        let path = resolve_bash_program(env).ok_or_else(|| ScriptError::DependencyMissing {
            name: "bash".to_string(),
            hint: BASH_MISSING_HINT.to_string(),
        })?;
        Ok(Command::new(path))
    }
    #[cfg(not(windows))]
    {
        Ok(match resolve_bash_program(env) {
            Some(path) => Command::new(path),
            None => Command::new("bash"),
        })
    }
}

/// Return the injected `PATH` value from `env`, then resolve `program` against
/// it. An exact `PATH` key is preferred because that is what Unix exec lookup
/// uses; if absent, fall back to a case-insensitive match so Windows-style
/// `Path` remains useful. Within each key class, last write wins, matching
/// `cmd.env` semantics.
pub(crate) fn resolve_interpreter(program: &str, env: &[(String, String)]) -> Option<PathBuf> {
    let injected_path = path_value(env)?;
    let resolved = resolve_program_in_path(program, injected_path);
    #[cfg(windows)]
    if program.eq_ignore_ascii_case("bash") {
        return resolved.filter(|path| !is_wsl_launcher(path));
    }
    resolved
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
        for candidate_name in executable_candidate_names(program) {
            let candidate = dir.join(candidate_name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn executable_candidate_names(program: &str) -> Vec<String> {
    if Path::new(program).extension().is_some() {
        return vec![program.to_string()];
    }
    let mut names = vec![program.to_string()];
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    for ext in pathext.split(';') {
        let ext = ext.trim();
        if ext.is_empty() {
            continue;
        }
        let ext = if ext.starts_with('.') {
            ext.to_string()
        } else {
            format!(".{ext}")
        };
        names.push(format!("{program}{ext}"));
    }
    names
}

#[cfg(not(windows))]
fn executable_candidate_names(program: &str) -> Vec<String> {
    vec![program.to_string()]
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

    #[cfg(windows)]
    fn normalized_windows_path(path: &Path) -> String {
        path.to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase()
    }

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
        assert!(exts.contains(&"lua"));
        assert_eq!(exts.len(), 5);
    }

    #[test]
    fn lua_resolves_last_so_existing_extensionless_lookups_do_not_change() {
        // This list doubles as the precedence order for `omakure run deploy`.
        // Appending `lua` last is what keeps `deploy.sh` winning over
        // `deploy.lua` for callers that predate this kind.
        let exts = script_extensions();
        assert_eq!(exts.last(), Some(&"lua"));
        let lua = exts.iter().position(|e| *e == "lua").unwrap();
        for earlier in ["bash", "sh", "ps1", "py"] {
            assert!(exts.iter().position(|e| *e == earlier).unwrap() < lua);
        }
    }

    #[test]
    fn lua_builds_a_self_exec_command_without_touching_the_interpreter_path() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("job.lua");
        std::fs::write(&script, "print('x')").unwrap();

        let command = command_for_script_with_env(&script, &[]).unwrap();
        assert_eq!(
            command.get_program(),
            std::env::current_exe().unwrap().as_os_str(),
            "Lua must re-execute this binary rather than resolve an interpreter"
        );
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, vec![LUA_HOST_ARG.as_ref(), script.as_os_str()]);
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
    #[cfg(windows)]
    #[test]
    fn resolve_bash_program_skips_wsl_launchers_and_uses_git_bash() {
        let root = tempfile::tempdir().unwrap();
        let system32_dir = root.path().join("Windows").join("System32");
        let sysnative_dir = root.path().join("Windows").join("Sysnative");
        let git_dir = root.path().join("Git").join("bin");
        for dir in [&system32_dir, &sysnative_dir, &git_dir] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let system32_bash = system32_dir.join("bash.exe");
        let sysnative_bash = sysnative_dir.join("bash.exe");
        let git = git_dir.join("bash.exe");
        std::fs::write(&system32_bash, "wsl launcher").unwrap();
        std::fs::write(&sysnative_bash, "wsl launcher").unwrap();
        std::fs::write(&git, "git bash").unwrap();

        // Exercise case-insensitive launcher classification without relying on
        // case-mutated filesystem paths.
        assert!(is_wsl_launcher(Path::new(r"C:\WINDOWS\SYSTEM32\BASH.EXE")));
        assert!(is_wsl_launcher(Path::new(r"C:\Windows\SYSNATIVE\BASH.EXE")));

        let env = vec![(
            "PATH".to_string(),
            format!(
                "{};{};{}",
                system32_dir.display(),
                sysnative_dir.display(),
                git_dir.display()
            ),
        )];

        let resolved = resolve_bash_program(&env).expect("Git Bash should be accepted");
        assert_eq!(
            normalized_windows_path(&resolved),
            normalized_windows_path(&git)
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_bash_program_returns_none_when_only_wsl_launcher_is_available() {
        let root = tempfile::tempdir().unwrap();
        let wsl_dir = root.path().join("Windows").join("System32");
        std::fs::create_dir_all(&wsl_dir).unwrap();
        std::fs::write(wsl_dir.join("bash.exe"), "wsl launcher").unwrap();

        let env = vec![("PATH".to_string(), wsl_dir.display().to_string())];
        assert_eq!(resolve_bash_program(&env), None);
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

    #[cfg(windows)]
    #[test]
    fn resolve_program_in_path_finds_windows_exe_suffix_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("python.EXE");
        std::fs::write(&exe, "shim").unwrap();
        let path_var = dir.path().display().to_string();

        let resolved = resolve_program_in_path("python", &path_var).expect("python.EXE found");

        assert_eq!(
            normalized_windows_path(&resolved),
            normalized_windows_path(&exe)
        );
    }

    #[cfg(windows)]
    #[test]
    fn executable_candidate_names_keep_explicit_exe_suffix_deterministic() {
        let candidates = executable_candidate_names("python.EXE");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].to_ascii_uppercase(), "PYTHON.EXE");
    }

    #[cfg(windows)]
    #[test]
    fn resolve_program_in_path_finds_windows_powershell_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("powershell.exe");
        std::fs::write(&exe, "shim").unwrap();
        let path_var = dir.path().display().to_string();

        let resolved =
            resolve_program_in_path("powershell", &path_var).expect("powershell.exe found");

        assert_eq!(
            normalized_windows_path(&resolved),
            normalized_windows_path(&exe)
        );
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
