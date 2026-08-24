# Headless migration

The current baseline is a deliberate breaking release. Omakure is CLI + HTTP
only; no interactive terminal application is hidden behind a default command.
Historical release notes describe the releases that contained the removed
features and are intentionally unchanged.

## Removed current surfaces

- No-argument TUI startup and all TUI screens, keybindings, forms, and widgets.
- The positional `omakure PATH` session scripts-root mode. Use
  `--scripts-dir PATH` or `OMAKURE_SCRIPTS_DIR`; the selected root now owns all
  workspace metadata.
- The `theme` command, TOML theme configuration, built-in `themes/` assets,
  Omarchy theme import, and theme-only spinner dependencies.
- Directory `index.lua` widgets and their Lua runtime dependency. `.lua` files
  are not a supported script extension in this baseline.
- TUI-only abstractions, snapshots, and UI documentation.

These are removals, not compatibility aliases. Do not restore them in docs,
completions, examples, or new code. `omakure --help` and `omakure help-ai` are
the source of truth for the current command surface.

## Migration steps

1. Replace `omakure .`, `omakure PATH`, and equivalent positional invocations
   with `omakure --scripts-dir PATH <command>`.
2. Replace interactive browsing/form workflows with `scripts`, `describe`,
   `init`, `run`, `queue`, `history`, and their HTTP operation equivalents.
3. Replace theme configuration and `~/.config/omakure/` theme assets with the
   terminal or client application's own presentation configuration.
4. Remove directory `index.lua` files from Omakure workspace assumptions; put
   any needed metadata in schemas, scripts, or the consuming client.
5. For automation and deployment, use `engine` with `/v1/health` and
   `/v1/ready`, or run `api`, `queue worker`, and `serve` as explicit processes.

## Data and release notes

The run database remains `.history/runs.sqlite` and the workspace environment
layout remains `.omakure/envs/`. The release package is now one binary per
platform; it does not contain themes, widgets, or workspace scripts. Back up
workspace state before upgrading because the existing run-state migration and
legacy JSON cleanup are destructive as documented in `ai-interface.md`.
