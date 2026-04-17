use std::path::Path;
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
    let kind = script_kind(script).ok_or(ScriptError::UnsupportedType)?;
    let mut command = match kind {
        ScriptKind::Bash => Command::new("bash"),
        ScriptKind::PowerShell => Command::new(powershell_program()),
        ScriptKind::Python => Command::new(python_program()),
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
}
