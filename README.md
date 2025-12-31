# omakure

Rust TUI to navigate a curated automation workspace, render optional folder widgets, and run
scripts that describe their parameters with a JSON schema. You organize folders, Omakure builds
the navigation, and the engine collects required values in a guided flow with execution history.

## Requirements

- Rust toolchain (development only)
- Git (Windows users: install Git for Windows so `git` and `bash` are on PATH)
- Bash (for `.bash`/`.sh` scripts)
- PowerShell (optional, for `.ps1` scripts)
- Python (optional, for `.py` scripts)
- `jq`

## Windows/macOS notes

- Windows: use Git for Windows (Git Bash) or WSL; ensure `git`, `bash`, and `jq` are in PATH.
- macOS: install `bash` and `jq` with Homebrew if missing.
- Scripts must use LF line endings (CRLF can break bash).
- Prefer Windows Terminal/PowerShell or Git Bash; CMD may not render the TUI well.
- Quote paths with spaces in scripts (e.g. `"C:\\Users\\Name\\Documents"`).

## Installation (one command, GitHub Releases)

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.sh | bash -s -- --repo This-Is-NPC/omakure
```

Windows (PowerShell):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.ps1 | iex"
```

Then run:

```bash
omakure
```

The installation creates the scripts folder at `~/Documents/omakure-scripts` (Windows: `%USERPROFILE%\Documents\omakure-scripts`).

## Workspace layout

Omakure treats the workspace as a filesystem score:

```
omakure-scripts/
├── .omaken/        # Curated flavors managed by Omakure
├── .history/       # Execution logs
├── omakure.toml    # Optional workspace config
└── azure/
    ├── index.lua   # Optional folder widget
    ├── rg-list-all.bash
    ├── rg-details.bash
    └── rg-delete.bash
```

If a folder includes `index.lua`, Omakure renders it in the TUI instead of the entries list.

## Update

```bash
omakure update
```

Linux/macOS requires `curl` (or `wget`) and `tar` for the update flow. Windows uses PowerShell.
The update also syncs new scripts from the repo without overwriting existing files.

Optional overrides:

```bash
omakure update --version v0.1.0 --repo This-Is-NPC/omakure
```

## Uninstall

```bash
omakure uninstall
```

To remove the scripts folder as well:

```bash
omakure uninstall --scripts
```

## Doctor

```bash
omakure doctor
```

Alias: `omakure check`

## List scripts

```bash
omakure scripts
```

Lists scripts recursively across the workspace (including `.omaken`).

## Run a script without the TUI

```bash
omakure run azure/rg-list-all
omakure run tools/cleanup
omakure run scripts/cleanup.py -- --force
```

## Init a new script template

```bash
omakure init my-script
omakure init tools/cleanup.py
```

## Config / env

```bash
omakure config
omakure env
```

## Omaken flavors

```bash
omakure list
omakure install <git-url>
omakure install <git-url> --name my-flavor
```

## Shell completion

```bash
omakure completion bash
omakure completion zsh
omakure completion fish
omakure completion pwsh
```

## Install a specific version

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.sh | VERSION=v0.1.0 bash -s -- --repo This-Is-NPC/omakure
```

Windows (PowerShell):

```powershell
$env:REPO = "This-Is-NPC/omakure"
$env:VERSION = "v0.1.0"
irm https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.ps1 | iex
```

## Release artifact format

Artifacts must follow the pattern below (version in the filename):

- `omakure-vX.Y.Z-linux-x86_64.tar.gz`
- `omakure-vX.Y.Z-linux-aarch64.tar.gz`
- `omakure-vX.Y.Z-darwin-x86_64.tar.gz`
- `omakure-vX.Y.Z-darwin-aarch64.tar.gz`
- `omakure-vX.Y.Z-windows-x86_64.zip`
- `omakure-vX.Y.Z-windows-aarch64.zip`

Archives must contain the binary at the root of the archive:

- `omakure` (Linux/macOS)
- `omakure.exe` (Windows)

## Install from source (optional)

```bash
bash install-from-source.sh
```

## How to run in development

```bash
cargo run
```

Use the TUI to select a script, fill the fields, and run.

In debug builds, the app will use the repo `scripts/` folder if it exists.
To override the scripts location, set `OMAKURE_SCRIPTS_DIR=/path/to/scripts`.

## How it works (overview)

1) Scripts live anywhere under `~/Documents/omakure-scripts` (Windows: `%USERPROFILE%\Documents\omakure-scripts`) with `.bash`, `.sh`, `.ps1`, or `.py` extensions.
2) When `SCHEMA_MODE=1`, the script prints JSON with `Name`, `Description`, and `Fields`.
3) If a folder has `index.lua`, the TUI renders the widget instead of the entries list.
4) The TUI reads schemas, prompts for values, and runs the script with args.
5) Every execution is captured in `.history/`.

## Script index (examples)

| Script | Description |
| --- | --- |
| `scripts/azure/rg-list-all.bash` | List resource groups with CreatedAt, LastModified, and CreatedBy. |
| `scripts/azure/rg-details.bash` | Show resource group details and list resources with CreatedAt, LastModified, and CreatedBy. |
| `scripts/azure/rg-delete.bash` | Delete a resource group and all resources inside it. |

## How to create a new script (step by step)

You can generate a starter script with:

```bash
omakure init my-script
```

Pass an extension to choose the template (`.bash`, `.sh`, `.ps1`, `.py`). If omitted, `.bash` is used.

1) Copy the template below to `~/Documents/omakure-scripts/my-script.bash` (Windows: `%USERPROFILE%\Documents\omakure-scripts\my-script.bash`). Use `.ps1` or `.py` for other runtimes.
2) Edit the schema JSON (name, description, and fields).
3) Adjust defaults and argument parsing.
4) Write the main logic.
5) Test:
   - `SCHEMA_MODE=1 bash scripts/my-script.bash` (should print valid JSON)
   - `bash scripts/my-script.bash --your-param value`
   - `cargo run` and select the script in the TUI

## Script anatomy

A script needs 4 clear blocks:

1) **Schema**: JSON that the TUI uses to know which fields to ask for.
2) **Defaults**: variables with initial values.
3) **Args + prompts**: reads `--param value` and asks if missing.
4) **Main**: script logic.

### Schema fields (JSON)

- `Name`: script identifier.
- `Description`: short description of what it does.
- `Fields`: list of fields for the TUI.

For each field in `Fields`:

- `Name`: internal field name.
- `Prompt`: text shown to the user.
- `Type`: `string`, `number`, or `bool`.
- `Order`: display order.
- `Required`: `true` or `false`.
- `Arg`: CLI argument name (e.g., `--target`).
- `Default`: default value (optional).
- `Choices`: list of allowed values (optional).

## Simple template (copy and paste)

```bash
#!/usr/bin/env bash
set -euo pipefail

# 1) Schema for the TUI
if [[ "${SCHEMA_MODE:-}" == "1" ]]; then
  cat <<'JSON'
{
  "Name": "my_script",
  "Description": "Describe what this script does.",
  "Fields": [
    {
      "Name": "target",
      "Prompt": "Target (optional)",
      "Type": "string",
      "Order": 1,
      "Required": false,
      "Arg": "--target"
    }
  ]
}
JSON
  exit 0
fi

# 2) Defaults
TARGET=""

# 3) Args + prompts
prompt_if_empty() {
  local var_name="$1"
  local label="$2"
  local value="${!var_name:-}"
  if [[ -z "${value}" ]]; then
    read -r -p "${label}: " value
    printf -v "${var_name}" '%s' "${value}"
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      TARGET="${2:-}"
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
printf "Running with target=%s\n" "${TARGET}"
```

## Architecture (Rust code)

The code follows ports-and-adapters:

- `src/domain`: schema parsing and input normalization
- `src/ports`: traits for repository and runner
- `src/use_cases`: use case orchestration
- `src/adapters`: TUI, filesystem, script runners, system checks
- `src/workspace`: workspace layout helpers
- `src/history`: execution log persistence
