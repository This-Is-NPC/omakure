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
- `.omaken/`, `.omaken/envs/`, and `omakure.toml` are never created
  inside `PATH` as a side effect of opening it.
- The Environments screen (`Ctrl+/` then `e`) continues to list and
  modify the global `.omaken/envs/` directory only.

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

Lists scripts recursively across the workspace (including `.omaken`).

## Run a script without the TUI

```bash
omakure run .omaken/azure/rg-list-all
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

## Omaken flavors

```bash
omakure list
omakure install <git-url>
omakure install <git-url> --name my-flavor
```

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
