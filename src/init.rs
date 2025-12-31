use crate::runtime::{script_extensions, script_kind, ScriptKind};
use crate::workspace::Workspace;
use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub struct InitOptions {
    pub name: String,
    pub scripts_dir: PathBuf,
}

pub fn print_init_help() {
    println!(
        "Usage: omakure init <script-path>\n\n\
Examples:\n\
  omakure init rg-list-all\n\
  omakure init tools/cleanup.py\n\n\
Notes:\n\
  Script paths are relative to the workspace root.\n\
  Extensions supported: .bash, .sh, .ps1, .py\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Scripts directory override\n\
  OVERTURE_SCRIPTS_DIR  Legacy scripts directory override\n\
  CLOUD_MGMT_SCRIPTS_DIR  Legacy scripts directory override"
    );
}

pub fn parse_init_args(
    args: &[String],
    scripts_dir: PathBuf,
) -> Result<InitOptions, Box<dyn Error>> {
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

pub fn run_init(options: InitOptions) -> Result<(), Box<dyn Error>> {
    let name = options.name.trim();
    if name.is_empty() {
        return Err("Script name cannot be empty".into());
    }
    let relative_path = ensure_script_path(name)?;

    let workspace = Workspace::new(options.scripts_dir.clone());
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
    let script_kind = script_kind(&script_path).ok_or("Unsupported script extension")?;
    let content = build_template(&script_id, script_kind);
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
        return Err(format!(
            "Unsupported extension. Allowed: {}",
            allowed
        )
        .into());
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
        "#!/usr/bin/env bash\n\
set -euo pipefail\n\
\n\
# 1) Schema for the TUI\n\
if [[ \"${{SCHEMA_MODE:-}}\" == \"1\" ]]; then\n\
  cat <<'JSON'\n\
{{\n\
  \"Name\": \"{script_id}\",\n\
  \"Description\": \"Describe what this script does.\",\n\
  \"Fields\": [\n\
    {{\n\
      \"Name\": \"target\",\n\
      \"Prompt\": \"Target (optional)\",\n\
      \"Type\": \"string\",\n\
      \"Order\": 1,\n\
      \"Required\": false,\n\
      \"Arg\": \"--target\"\n\
    }}\n\
  ]\n\
}}\n\
JSON\n\
  exit 0\n\
fi\n\
\n\
# 2) Defaults\n\
TARGET=\"\"\n\
\n\
# 3) Args + prompts\n\
prompt_if_empty() {{\n\
  local var_name=\"$1\"\n\
  local label=\"$2\"\n\
  local value=\"${{!var_name:-}}\"\n\
  if [[ -z \"${{value}}\" ]]; then\n\
    read -r -p \"${{label}}: \" value\n\
    printf -v \"${{var_name}}\" '%s' \"${{value}}\"\n\
  fi\n\
}}\n\
\n\
while [[ $# -gt 0 ]]; do\n\
  case \"$1\" in\n\
    --target)\n\
      TARGET=\"${{2:-}}\"\n\
      shift 2\n\
      ;;\n\
    *)\n\
      echo \"Unknown arg: $1\" >&2\n\
      exit 1\n\
      ;;\n\
  esac\n\
done\n\
\n\
prompt_if_empty TARGET \"Target (optional)\"\n\
\n\
# 4) Main\n\
echo \"TODO: implement {script_id}\"\n",
        script_id = script_id
    )
}

fn build_powershell_template(script_id: &str) -> String {
    format!(
        "# PowerShell script template\n\
\n\
if ($env:SCHEMA_MODE -eq \"1\") {{\n\
@'\n\
{{\n\
  \"Name\": \"{script_id}\",\n\
  \"Description\": \"Describe what this script does.\",\n\
  \"Fields\": [\n\
    {{\n\
      \"Name\": \"target\",\n\
      \"Prompt\": \"Target (optional)\",\n\
      \"Type\": \"string\",\n\
      \"Order\": 1,\n\
      \"Required\": false,\n\
      \"Arg\": \"--target\"\n\
    }}\n\
  ]\n\
}}\n\
'@\n\
  exit 0\n\
}}\n\
\n\
$Target = \"\"\n\
for ($i = 0; $i -lt $args.Length; $i++) {{\n\
  switch ($args[$i]) {{\n\
    \"--target\" {{\n\
      $Target = $args[$i + 1]\n\
      $i++\n\
    }}\n\
    default {{\n\
      Write-Error \"Unknown arg: $($args[$i])\"\n\
      exit 1\n\
    }}\n\
  }}\n\
}}\n\
\n\
if (-not $Target) {{\n\
  $Target = Read-Host \"Target (optional)\"\n\
}}\n\
\n\
Write-Output \"TODO: implement {script_id}\"\n",
        script_id = script_id
    )
}

fn build_python_template(script_id: &str) -> String {
    format!(
        "#!/usr/bin/env python3\n\
import json\n\
import os\n\
import sys\n\
import argparse\n\
\n\
if os.environ.get(\"SCHEMA_MODE\") == \"1\":\n\
    print(json.dumps({{\n\
        \"Name\": \"{script_id}\",\n\
        \"Description\": \"Describe what this script does.\",\n\
        \"Fields\": [\n\
            {{\n\
                \"Name\": \"target\",\n\
                \"Prompt\": \"Target (optional)\",\n\
                \"Type\": \"string\",\n\
                \"Order\": 1,\n\
                \"Required\": False,\n\
                \"Arg\": \"--target\"\n\
            }}\n\
        ]\n\
    }}, indent=2))\n\
    sys.exit(0)\n\
\n\
parser = argparse.ArgumentParser()\n\
parser.add_argument(\"--target\", default=\"\")\n\
args = parser.parse_args()\n\
target = args.target or input(\"Target (optional): \")\n\
\n\
print(f\"TODO: implement {script_id}\")\n",
        script_id = script_id
    )
}

#[cfg(not(windows))]
fn set_executable_permissions(path: &Path) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(windows)]
fn set_executable_permissions(_path: &Path) -> Result<(), Box<dyn Error>> {
    Ok(())
}
