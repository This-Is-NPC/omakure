# How to create a Script

You can generate a starter script with:

```bash
omakure init my-script
```

Pass an extension to choose the template (`.bash`, `.sh`, `.ps1`, `.py`). If omitted, `.bash` is used.

## Step by step

1) Copy the template below to `~/Documents/omakure-scripts/my-script.bash` (Windows: `%USERPROFILE%\Documents\omakure-scripts\my-script.bash`). Use `.ps1` or `.py` for other runtimes.
2) Edit the schema JSON (name, description, and fields) inside the schema block.
3) Adjust defaults and argument parsing.
4) Write the main logic.
5) Test:
   - `bash scripts/my-script.bash --your-param value`
   - `cargo run` and select the script in the TUI

## Script anatomy

A script needs 4 clear blocks:

1) **Schema**: JSON block the TUI uses to know which fields to ask for, between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`.
2) **Defaults**: variables with initial values.
3) **Args + prompts**: reads `--param value` and asks if missing.
4) **Main**: script logic.

## Schema fields (JSON)

- `Name`: script identifier.
- `Description`: short description of what it does.
- `Fields`: list of fields for the TUI.
- `Outputs`: values the script produces (optional).
- `Queue`: queue configuration for batch runs (optional).

Outputs and Queue details render in the schema preview panel in the TUI.

For each field in `Fields`:

- `Name`: internal field name.
- `Prompt`: text shown to the user.
- `Type`: `string`, `number`, or `bool`.
- `Order`: display order.
- `Required`: `true` or `false`.
- `Arg`: CLI argument name (e.g., `--target`).
- `Default`: default value (optional).
- `Choices`: list of allowed values (optional).

### Outputs (optional)

Each output uses:

- `Name`: output name.
- `Type`: output type (`string`, `number`, `bool`).

### Queue (optional)

Queue supports either `Matrix` or `Cases`:

- `Matrix`: list of values to combine. Each entry uses `Name` and `Values`.
- `Cases`: list of explicit value sets. Each case can have an optional `Name` and a `Values` array of `Name`/`Value` pairs.

## Comment prefixes

- `.bash`/`.sh`: `#`
- `.ps1`: `#` or `;`
- `.py`: `#`

## Simple template (copy and paste)

```bash
#!/usr/bin/env bash
set -euo pipefail

# 1) Schema for the TUI
# OMAKURE_SCHEMA_START
# {
#   "Name": "my_script",
#   "Description": "Describe what this script does.",
#   "Fields": [
#     {
#       "Name": "target",
#       "Prompt": "Target (optional)",
#       "Type": "string",
#       "Order": 1,
#       "Required": false,
#       "Arg": "--target"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

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

