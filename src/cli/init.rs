use crate::runtime::{script_extensions, script_kind, ScriptKind};
use crate::util::set_executable_permissions;
use crate::workspace::Workspace;
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::ENV_HELP;

pub struct InitOptions {
    pub name: String,
    pub scripts_dir: PathBuf,
}

pub fn print_help() {
    println!(
        "Usage: omakure init <script-path>\n\n\
Examples:\n\
  omakure init rg-list-all\n\
  omakure init tools/cleanup.py\n\n\
Notes:\n\
  Script paths are relative to the workspace root.\n\
  Extensions supported: .bash, .sh, .ps1, .py\n\n\
{ENV_HELP}"
    );
}

pub fn parse_args(args: &[String], scripts_dir: PathBuf) -> Result<InitOptions, Box<dyn Error>> {
    if args.is_empty() {
        return Err("Missing script name. Use `omakure init <script-name>`.".into());
    }

    let mut name = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                let value = args.get(i + 1).ok_or("Missing value for --name")?;
                name = Some(value.to_string());
                i += 2;
            }
            value if name.is_none() => {
                name = Some(value.to_string());
                i += 1;
            }
            unknown => {
                return Err(format!("Unknown init arg: {}", unknown).into());
            }
        }
    }

    let name = name.ok_or("Missing script name")?;
    Ok(InitOptions { name, scripts_dir })
}

pub fn run(options: InitOptions) -> Result<(), Box<dyn Error>> {
    let name = options.name.trim();
    if name.is_empty() {
        return Err("Script name cannot be empty".into());
    }
    let relative_path = ensure_script_path(name)?;

    let workspace = Workspace::new(options.scripts_dir);
    workspace.ensure_layout()?;
    let script_path = workspace.root().join(&relative_path);
    if script_path.exists() {
        return Err(format!("Script already exists: {}", script_path.display()).into());
    }
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let script_id = normalize_script_id(&script_path);
    if script_id.is_empty() {
        return Err("Script name must contain letters or numbers".into());
    }
    let kind = script_kind(&script_path).ok_or("Unsupported script extension")?;
    let content = build_template(&script_id, kind);
    fs::write(&script_path, content)?;
    set_executable_permissions(&script_path)?;

    println!("Created {}", script_path.display());
    Ok(())
}

fn ensure_script_path(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut path = PathBuf::from(name);
    if path.is_absolute() {
        return Err("Script name must be a relative path".into());
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Script name must not include parent or root components".into());
            }
            _ => {}
        }
    }
    if path.extension().is_none() {
        path.set_extension("bash");
    }
    if script_kind(&path).is_none() {
        let allowed = script_extensions().join(", ");
        return Err(format!("Unsupported extension. Allowed: {}", allowed).into());
    }
    Ok(path)
}

fn normalize_script_id(path: &Path) -> String {
    let trimmed = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let mut out = String::new();
    let mut prev_underscore = false;
    for ch in trimmed.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    out.trim_matches('_').to_string()
}

fn build_template(script_id: &str, kind: ScriptKind) -> String {
    match kind {
        ScriptKind::Bash => build_bash_template(script_id),
        ScriptKind::PowerShell => build_powershell_template(script_id),
        ScriptKind::Python => build_python_template(script_id),
    }
}

fn build_bash_template(script_id: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

# 1) Schema for the TUI
# OMAKURE_SCHEMA_START
# {{
#   "Name": "{script_id}",
#   "Description": "Describe what this script does.",
#   "Tags": [],
#   "Fields": [
#     {{
#       "Name": "target",
#       "Prompt": "Target (optional)",
#       "Type": "string",
#       "Order": 1,
#       "Required": false,
#       "Arg": "--target"
#     }}
#   ]
# }}
# OMAKURE_SCHEMA_END


# 2) Defaults
TARGET=""

# 3) Args + prompts
prompt_if_empty() {{
  local var_name="$1"
  local label="$2"
  local value="${{!var_name:-}}"
  if [[ -z "${{value}}" ]]; then
    read -r -p "${{label}}: " value
    printf -v "${{var_name}}" '%s' "${{value}}"
  fi
}}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      TARGET="${{2:-}}"
      shift 2
      ;;
    *)
      echo "Unknown arg: $1" >&2
      exit 1
      ;;
  esac
done

prompt_if_empty TARGET "Target (optional)"

# 4) Main
echo "TODO: implement {script_id}"
"#,
        script_id = script_id
    )
}

fn build_powershell_template(script_id: &str) -> String {
    format!(
        r#"# PowerShell script template

# OMAKURE_SCHEMA_START
# {{
#   "Name": "{script_id}",
#   "Description": "Describe what this script does.",
#   "Tags": [],
#   "Fields": [
#     {{
#       "Name": "target",
#       "Prompt": "Target (optional)",
#       "Type": "string",
#       "Order": 1,
#       "Required": false,
#       "Arg": "--target"
#     }}
#   ]
# }}
# OMAKURE_SCHEMA_END

$Target = ""
for ($i = 0; $i -lt $args.Length; $i++) {{
  switch ($args[$i]) {{
    "--target" {{
      $Target = $args[$i + 1]
      $i++
    }}
    default {{
      Write-Error "Unknown arg: $($args[$i])"
      exit 1
    }}
  }}
}}

if (-not $Target) {{
  $Target = Read-Host "Target (optional)"
}}

Write-Output "TODO: implement {script_id}"
"#,
        script_id = script_id
    )
}

fn build_python_template(script_id: &str) -> String {
    format!(
        r#"#!/usr/bin/env python3
import argparse

# OMAKURE_SCHEMA_START
# {{
#   "Name": "{script_id}",
#   "Description": "Describe what this script does.",
#   "Tags": [],
#   "Fields": [
#     {{
#       "Name": "target",
#       "Prompt": "Target (optional)",
#       "Type": "string",
#       "Order": 1,
#       "Required": false,
#       "Arg": "--target"
#     }}
#   ]
# }}
# OMAKURE_SCHEMA_END

parser = argparse.ArgumentParser()
parser.add_argument("--target", default="")
args = parser.parse_args()
target = args.target or input("Target (optional): ")

print(f"TODO: implement {script_id}")
"#,
        script_id = script_id
    )
}
