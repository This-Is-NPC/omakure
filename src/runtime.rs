use std::error::Error;
use std::path::Path;
use std::process::Command;

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

pub fn command_for_script(script: &Path) -> Result<Command, Box<dyn Error>> {
    let kind = script_kind(script).ok_or("Unsupported script type")?;
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
