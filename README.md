# omakure

Rust TUI and CLI for navigating, running, **scheduling**, and auditing automation scripts.
You organize folders, Omakure builds the navigation, collects required values via a JSON
schema each script embeds, and records every run in a SQLite-backed state machine.

Scripts can opt into a `Schedule` block to become self-contained automation units driven
by the built-in `omakure serve` cron daemon (`trigger = Scheduled`); everything still flows
through the same queue/worker/history pipeline as manual runs.

## Requirements

- Rust toolchain (development only)
- Git (Windows users: install Git for Windows so `git` and `bash` are on PATH)
- Bash (for `.bash`/`.sh` scripts)
- PowerShell (optional, for `.ps1` scripts)
- Python (optional, for `.py` scripts)
- `jq`

### Windows/macOS notes

- Windows: use Git for Windows (Git Bash) or WSL; ensure `git`, `bash`, and `jq` are in PATH.
- macOS: install `bash` and `jq` with Homebrew if missing.
- Scripts must use LF line endings (CRLF can break bash).
- Prefer Windows Terminal/PowerShell or Git Bash; CMD may not render the TUI well.
- Quote paths with spaces in scripts (e.g. `"C:\\Users\\Name\\Documents"`).

## Quick start

1) Install from releases:

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.sh | bash -s -- --repo This-Is-NPC/omakure
```

Windows (PowerShell):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.ps1 | iex"
```

2) Run:

```bash
# Open the TUI against the global workspace
omakure

# Or open the TUI against any directory (session-only scripts root)
omakure .
omakure ../team-scripts
omakure /abs/path/to/scripts
```

The positional path only changes which directory the TUI browses for the
current session. History, environments, the search index, and
`omakure.toml` always stay in the global workspace — Omakure never
creates `.omakure/`, `.history/`, or `omakure.toml` inside the directory
you point it at.

3) Put scripts under `~/Documents/omakure-scripts` (Windows: `%USERPROFILE%\Documents\omakure-scripts`). Omakure scans this tree for `.bash`, `.sh`, `.ps1`, and `.py` scripts, while skipping Omakure-owned metadata under `.omakure/`. To hide helpers, fixtures, or vendored folders from the TUI, `omakure scripts`, search, and the scheduler, add `.omakureignore` files at the scripts root or inside child folders.

4) Make the script visible to Omakure by embedding a schema JSON block between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`. The `omakure init my-script` command generates a template with the schema block.

## Advanced

- Change the default scripts path: `.docs/scripts-path.md`
- Exclude files from scanning with `.omakureignore`: `.docs/scripts-path.md`
- Environment documents and defaults: `.docs/environments.md`
- Attach reusable script repositories with Batteries: `.docs/batteries.md`
- Internal HTTP management API contract: `.docs/http-api.md`
- Lua widgets (`index.lua`): `.docs/lua-widgets.md`

## Using Omakure from an AI agent

Omakure exposes a low-token, machine-readable CLI surface so AI agents
can list, describe, create, run, and audit scripts as fluently as `git`
or `gh`. Every AI-relevant verb supports `--json` and emits a stable
envelope `{ ok, data, error, schema_version }`. Run history is
persisted in `<workspace>/.history/runs.sqlite` and is queryable via
`omakure history`.

A queue + worker daemon (`omakure queue add`, `omakure queue worker`)
and a structured trace stream (`omakure trace` from inside a script,
`omakure history traces` from outside) make Omakure usable as the
orchestration core of a fleet of independent AI agents.

```bash
# One-call discovery
omakure help-ai

# Create a script and run it under an AI actor
omakure --json init my-task.sh --schema-json '{"Name":"my_task","Fields":[]}' --body-stdin <<'BODY'
#!/usr/bin/env bash
omakure trace "started" --level info
echo "hi"
BODY
omakure --json run my-task --actor ai --reason "smoke test"

# Or push it onto the queue and let a worker drain it
omakure --json queue add my-task --actor agent-sp --priority 10
omakure queue worker --concurrency 4 &

# Watch and audit
omakure --json history list --state running
omakure --json history traces $RUN_ID
omakure --json history stats
```

## Batteries

Batteries attach reusable Omakure-compatible script repositories without
executing them directly from a Git clone. A Battery is registered, synced into
`.omakure/batteries/cache/`, inspected as untrusted input, and only selected
scripts are copied into the trusted scripts workspace with provenance metadata.

```bash
omakure --json battery list
omakure --json battery add https://example.invalid/azure.git --name azure --ref main
omakure --json battery sync azure
omakure --json battery inspect azure
omakure --json battery scripts azure
omakure --json battery install azure azure.rg-list-all
omakure --json battery install azure azure.rg-list-all --force
omakure --json battery remove azure --remove-cache
```

Battery clones are not executable workspace content. Omakure validates manifest
paths, script extensions, schema blocks, symlinks, and traversal before install.
HTTP Battery endpoints call the same Battery operations used by the CLI, with
HTTP registration restricted to `https://` sources. Full contract:
`.docs/batteries.md`.

## HTTP API

The internal HTTP management API is available with `omakure api` and specified
in `.docs/http-api.md`. It wraps the same shared operations as the CLI and is
not a public internet API: default bind is loopback, non-loopback requires
explicit opt-in, and every endpoint except health requires a Bearer token.

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api

curl http://127.0.0.1:7878/v1/health
curl -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/scripts
```

Use `--allow-non-loopback` only for trusted internal/container networks. V1 has
no browser CORS support, OAuth/RBAC, or public-internet threat model. HTTP
Battery registration accepts `https://` sources only; local Battery sources are
CLI-only.

The API exposes config, doctor, search, safe tree/content browsing, scripts,
runs/queue, and Battery management through `src/operations/*`; route handlers
must stay adapters, not second implementations.

> **Upgrading from an older release deletes legacy `.history/*.json`
> files and rebuilds `runs.sqlite` if its schema is older than the
> state-machine release.** Back up `.history/` first if you care about
> historical run data. See `.docs/ai-interface.md` for the full
> contract.

Full reference: `.docs/ai-interface.md`.

## Documentation

- AI agent interface: `.docs/ai-interface.md`
- Installation, updates, and uninstall: `.docs/installation.md`
- Workspace layout and defaults: `.docs/workspace.md`
- Scripts path overrides: `.docs/scripts-path.md`
- Environment documents: `.docs/environments.md`
- Batteries: `.docs/batteries.md`
- HTTP API contract: `.docs/http-api.md`
- Lua widgets (`index.lua`): `.docs/lua-widgets.md`
- CLI usage: `.docs/usage.md`
- Scheduled tasks (cron daemon): `.docs/scheduling.md`
- How to create a script: `.docs/how-to-create-a-script.md`
- How it works (overview + examples): `.docs/how-it-works.md`
- Development guide: `.docs/development.md`
- Release artifacts: `.docs/release-artifacts.md`

## License

AGPL-3.0-only. See `LICENSE`.
