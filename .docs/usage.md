# CLI usage

## Open the TUI

```bash
omakure                       # browse the global workspace
omakure .                     # browse the current directory
omakure ../team-scripts       # browse a relative path
omakure /abs/path/to/scripts  # browse an absolute path
```

The optional positional `PATH` argument is a **session-only scripts root
override**. While the TUI is open it lists scripts and loads
`<PATH>/index.lua` from that directory, but every other piece of
Omakure state stays in the global workspace:

- `.history/` and the SQLite search index always live in the global
  workspace and never under `PATH`.
- `.omakure/`, `.omakure/envs/`, and `omakure.toml` are never created
  inside `PATH` as a side effect of opening it.
- The Environments screen (`Ctrl+/` then `e`) continues to list and
  modify the global `.omakure/envs/` directory only.

The positional path is mutually exclusive with `--scripts-dir`. If both
are supplied, Omakure exits before launching the TUI with a clear error.

If `<PATH>` does not exist or is a regular file (not a directory),
Omakure exits with a deterministic error and does not start the TUI.

## Doctor

```bash
omakure doctor
```

Alias: `omakure check`

## List scripts

```bash
omakure scripts
```

Lists scripts recursively across the workspace, skipping Omakure-owned metadata under `.omakure/`.

## Run a script without the TUI

```bash
omakure run azure/rg-list-all
omakure run tools/cleanup
omakure run scripts/cleanup.py -- --force
```

### Per-run env injection (`--env-file`)

Pass an extra env file for a single run without touching the managed
active env:

```bash
omakure run scripts/deploy.py --env-file ./prod.env
```

The file is parsed with the case-preserving injector and merged on top of
the managed active env, so its keys override the active env for that run
(precedence: active env < `--env-file` < Omakure-reserved). A path that
cannot be read is a hard error. See `environments.md` for the injection
model and `env-injection-spec.md` for the full precedence table.

## Init a new script template

```bash
omakure init my-script
omakure init tools/cleanup.py
```

See `how-to-create-a-script.md` for the step-by-step guide and templates.

## Batteries

Batteries register reusable Omakure-compatible automation repositories. The Git
checkout is cached under `.omakure/batteries/cache/` and treated as untrusted;
Omakure never runs scripts directly from that cache. Install copies one
validated script into the trusted scripts workspace and records source
provenance under `.omakure/batteries/installed/`.

```bash
omakure battery list
omakure battery add https://example.invalid/azure.git --name azure --ref main
omakure battery sync azure
omakure battery inspect azure
omakure battery scripts azure
omakure battery install azure azure.rg-list-all
omakure battery install azure azure.rg-list-all --force
omakure battery remove azure --remove-cache
```

All Battery commands honor the global `--json` flag:

```bash
omakure --json battery scripts azure
```

Lifecycle:

- `add` records the source URL/ref/name; it does not execute repo content.
- `sync` clones/fetches, checks out a detached commit, validates the manifest,
  and records the resolved commit SHA.
- `inspect` reads and validates the synced manifest.
- `scripts` lists installable manifest entries that pass safety validation.
- `install` copies one script into the scripts workspace; existing targets are
  refused unless `--force` is set.
- `remove` unregisters the Battery; `--remove-cache` also deletes the cached
  clone. Installed scripts are left in the trusted workspace.

HTTP endpoints wrap the same operations backing these commands
(`list_batteries`, `add_battery`, `sync_battery`, `inspect_battery`,
`list_battery_scripts`, `install_battery_script`, `remove_battery`) rather than
shelling out or duplicating safety logic. HTTP Battery registration is stricter:
it accepts `https://` sources only.

See `batteries.md` for repository format, safety rules, install provenance, and
validation expectations.

## HTTP API

The internal HTTP management API runs in the same binary and wraps the same
shared operations as the CLI. V1 is scoped as a trusted management surface,
not a public internet API.

Loopback mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api
```

Internal container/private-network mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --bind 0.0.0.0:7878 --allow-non-loopback
```

Safety model:

- default bind is loopback (`127.0.0.1:7878`),
- non-loopback bind requires explicit opt-in,
- every endpoint except health requires `Authorization: Bearer <token>`,
- request bodies are limited to 1 MiB,
- route handlers call shared operations instead of CLI modules,
- no CORS/OAuth/RBAC/browser support is provided in v1,
- Battery cache scripts are never executed directly by HTTP.
- HTTP Battery registration accepts `https://` sources only; local paths and
  `file://` sources are CLI-only.

Quick check:

```bash
curl http://127.0.0.1:7878/v1/health
curl -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/scripts
```

Implemented endpoints:

```text
GET    /v1/health
GET    /v1/workspace
GET    /v1/scripts
GET    /v1/scripts/{script_id}
GET    /v1/scripts/{script_id}/schema
GET    /v1/runs
GET    /v1/runs/{run_id}
GET    /v1/runs/{run_id}/traces
GET    /v1/queue/stats
POST   /v1/runs
POST   /v1/runs/{run_id}/cancel
POST   /v1/runs/{run_id}/dead-letter
GET    /v1/batteries
POST   /v1/batteries
POST   /v1/batteries/{battery_id}/sync
GET    /v1/batteries/{battery_id}
GET    /v1/batteries/{battery_id}/scripts
POST   /v1/batteries/{battery_id}/scripts/{script_id}/install
DELETE /v1/batteries/{battery_id}
```

See `http-api.md` for JSON request bodies, error/status mapping, deployment
guidance, and the CLI/HTTP/shared-operation parity matrix.

## Config / env

```bash
omakure config
omakure env
```

`config` (alias `env`) prints resolved workspace paths, the active
environment's injected keys (sensitive values masked with `****`), and
the **absolute interpreter path** Omakure will execute against the active
env's `PATH` — so you can confirm which `python` actually runs. `--json`
includes `active_env_keys` and `interpreter`.

TUI notes:

- The Environments screen shows a preview panel for the selected env file.
- Preview scroll: `PgUp` / `PgDn`, `Home` / `End`.
- See `environments.md` for details.

## Themes

```bash
omakure theme list
omakure theme set <name>
omakure theme preview <name>
omakure theme path
```

- Global theme config: `~/.config/omakure/config.toml` with `[theme] name = "..."`.
- Built-in themes are copied to `~/.config/omakure/themes/` on first use.
- Workspace override: add `[theme] name = "..."` to `omakure.toml`.

## Removed Omaken flavors

The old Omaken flavor surface was removed. `omakure list` and
`omakure install <git-url>` no longer manage script packs. Reusable script
repositories are managed by the `omakure battery ...` command family instead.

## Scheduler (`omakure serve`)

Runs scripts declaring a `Schedule` block automatically.

```bash
omakure serve            # foreground (Ctrl+C stops)
omakure serve --detach   # detached daemon (Unix only)
omakure serve --stop     # SIGTERM + 5s grace
```

See `scheduling.md` for the full reference: cron formats, lifecycle,
overlap protection, systemd autostart, observability, and failure
modes.

## Shell completion

```bash
omakure completion bash
omakure completion zsh
omakure completion fish
omakure completion pwsh
```
