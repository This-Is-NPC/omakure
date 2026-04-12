use crate::cli::args::InitArgs;
use crate::cli::json::{self, codes};
use crate::domain::parse_schema;
use crate::runtime::{script_extensions, script_kind, ScriptKind};
use crate::util::set_executable_permissions;
use crate::workspace::Workspace;
use serde::Serialize;
use std::error::Error;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct InitPayload {
    pub absolute_path: String,
    pub relative_path: String,
    pub kind: String,
}

pub fn run_with_format(
    scripts_dir: PathBuf,
    options: InitArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let name = options
        .name
        .clone()
        .or_else(|| options.script.clone())
        .ok_or_else(|| "Missing script name. Use `omakure init <script-name>`.".to_string())?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Script name cannot be empty".into());
    }
    let relative_path = ensure_script_path(&name)?;

    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;
    let script_path = workspace.root().join(&relative_path);
    if script_path.exists() && !options.force {
        return emit_error(
            json_output,
            codes::SCRIPT_EXISTS,
            format!("Script already exists: {}", script_path.display()),
        );
    }
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let script_id = normalize_script_id(&script_path);
    if script_id.is_empty() {
        return Err("Script name must contain letters or numbers".into());
    }
    let kind = script_kind(&script_path).ok_or("Unsupported script extension")?;

    // Resolve the optional schema-json input.
    let schema_text = match options.schema_json.as_deref() {
        Some(s) => Some(load_schema_input(s)?),
        None => None,
    };
    if let Some(text) = &schema_text {
        // Validate before writing — never persist a script whose schema
        // would not parse.
        if let Err(err) = parse_schema(text) {
            return emit_error(
                json_output,
                codes::SCHEMA_INVALID,
                format!("schema-json invalid: {}", err),
            );
        }
    }

    // Read the optional body from stdin.
    let body = if options.body_stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Some(buf)
    } else {
        None
    };

    let content = if let Some(text) = schema_text {
        build_with_schema(&text, body.as_deref(), kind)
    } else {
        build_template(&script_id, kind)
    };
    fs::write(&script_path, content)?;
    set_executable_permissions(&script_path)?;

    if json_output {
        let payload = InitPayload {
            absolute_path: fs::canonicalize(&script_path)
                .unwrap_or_else(|_| script_path.clone())
                .to_string_lossy()
                .to_string(),
            relative_path: relative_path.to_string_lossy().to_string(),
            kind: match kind {
                ScriptKind::Bash => "bash".into(),
                ScriptKind::PowerShell => "powershell".into(),
                ScriptKind::Python => "python".into(),
            },
        };
        json::print_ok(payload);
        return Ok(());
    }

    println!("Created {}", script_path.display());
    Ok(())
}

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

/// Resolve the `--schema-json` value: either a literal JSON string, or a
/// `@path/to/file.json` reference.
fn load_schema_input(input: &str) -> Result<String, Box<dyn Error>> {
    if let Some(rest) = input.strip_prefix('@') {
        Ok(fs::read_to_string(rest)?)
    } else {
        Ok(input.to_string())
    }
}

/// Build a script file that embeds the supplied schema between the
/// `OMAKURE_SCHEMA_START` / `OMAKURE_SCHEMA_END` markers using the right
/// comment prefix for the script kind. Optionally writes a caller-supplied
/// body verbatim under the schema header.
fn build_with_schema(schema_text: &str, body: Option<&str>, kind: ScriptKind) -> String {
    let prefix = match kind {
        ScriptKind::Bash | ScriptKind::Python => "#",
        ScriptKind::PowerShell => "#",
    };
    let header = match kind {
        ScriptKind::Bash => "#!/usr/bin/env bash\nset -euo pipefail\n\n",
        ScriptKind::Python => "#!/usr/bin/env python3\n\n",
        ScriptKind::PowerShell => "",
    };
    let mut out = String::new();
    out.push_str(header);
    out.push_str(prefix);
    out.push_str(" OMAKURE_SCHEMA_START\n");
    for line in schema_text.lines() {
        out.push_str(prefix);
        if !line.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(prefix);
    out.push_str(" OMAKURE_SCHEMA_END\n");
    out.push('\n');
    if let Some(body) = body {
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_with_schema_embeds_schema_text_in_bash_with_hash_prefix() {
        let schema = r#"{"Name":"x","Fields":[]}"#;
        let out = build_with_schema(schema, None, ScriptKind::Bash);
        assert!(out.starts_with("#!/usr/bin/env bash"));
        assert!(out.contains("# OMAKURE_SCHEMA_START\n"));
        assert!(out.contains("# OMAKURE_SCHEMA_END\n"));
        // Schema body must be commented with the bash prefix.
        assert!(out.contains(r#"# {"Name":"x","Fields":[]}"#));
    }

    #[test]
    fn build_with_schema_appends_body() {
        let schema = r#"{"Name":"x","Fields":[]}"#;
        let body = "echo hi\n";
        let out = build_with_schema(schema, Some(body), ScriptKind::Bash);
        assert!(out.ends_with("echo hi\n"));
    }

    #[test]
    fn load_schema_input_inline_and_file() {
        let inline = load_schema_input(r#"{"Name":"x","Fields":[]}"#).unwrap();
        assert!(inline.contains("Name"));
        let dir =
            std::env::temp_dir().join(format!("omakure_init_test_schema_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("schema.json");
        fs::write(&path, r#"{"Name":"y","Fields":[]}"#).unwrap();
        let from_file = load_schema_input(&format!("@{}", path.display())).unwrap();
        assert!(from_file.contains("\"y\""));
        let _ = fs::remove_dir_all(&dir);
    }
}
