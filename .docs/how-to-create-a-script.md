# How to create a script

Generate a starter file with `omakure init`, or write a supported Bash,
PowerShell, or Python script directly. Omakure discovers it from the selected
workspace when the file extension and schema markers are valid.

```bash
omakure init tools/hello.sh
omakure --json scripts
omakure --json describe tools/hello.sh
omakure --json run tools/hello.sh --no-prompt
```

## Schema

Put a JSON object between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END` using
the comment prefix for the script extension:

```bash
# OMAKURE_SCHEMA_START
# {
#   "Name": "hello",
#   "Description": "Print a greeting",
#   "Tags": ["example"],
#   "Fields": [],
#   "Schedule": { "Cron": "@hourly", "Enabled": true }
# }
# OMAKURE_SCHEMA_END
```

`Fields` may contain `Name`, `Prompt`, `Type`, `Order`, `Required`, `Arg`,
`Default`, and `Choices`. Types are `string`, `number`, `bool`/`boolean`, and
`secret`; secret fields cannot declare choices. `Outputs` and `Queue` are
optional metadata used by the CLI/HTTP schema projections. `Schedule` accepts
the cron forms documented in `scheduling.md`.

Use `--schema-json '<json>|@file'` with `init` to validate and embed a schema,
and `--body-stdin` to supply the script body. Existing files require `--force`.

## Script contract

Scripts should parse their declared `Arg` values and remain usable outside
Omakure. Do not assume a form UI or a Lua directory widget exists. A script can
emit structured progress from an Omakure run:

```bash
omakure trace "started" --level info --data '{"target":"prod"}'
```

The executor supplies `OMAKURE_RUN_ID` to direct/queued/scheduled child
processes. `history traces RUN_ID` reads those events. Environment files and
secret references are described in `environments.md` and `ai-interface.md`.

## Comment prefixes

- `.bash`/`.sh`: `#`
- `.ps1`: `#` or `;`
- `.py`: `#`
- `.lua`: `--`

## Minimal Bash script

```bash
#!/usr/bin/env bash
set -euo pipefail

# OMAKURE_SCHEMA_START
# { "Name": "hello", "Description": "Print a greeting", "Fields": [] }
# OMAKURE_SCHEMA_END

printf 'hello from omakure\n'
```

Validate and run it through named, headless commands:

```bash
omakure --json doctor
omakure --json describe hello.sh
omakure --json run hello.sh --actor local --reason smoke
omakure --json history list --limit 5
```
