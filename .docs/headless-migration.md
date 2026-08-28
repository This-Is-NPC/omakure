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
- Directory `index.lua` widgets and their Lua runtime dependency. The widget
  runtime stays removed.

  `.lua` was not a supported script extension in this baseline, but it is now:
  roadmap item 5 added `.lua` as a first-class script kind executed by a Lua
  runtime embedded in the binary. That is a different Lua from the widget
  runtime above. A leftover `index.lua` in a workspace is therefore discovered
  as an ordinary script, and `describe` will fail on it for lack of a schema
  block, exactly as a schemaless `.sh` does today. Delete it, or give it a
  schema.
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
5. For automation and deployment, use `node serve` with `/v1/health` and
   `/v1/ready`, or run `api`, `queue worker`, and `serve` as explicit processes.

## Data and release notes

The run database remains `.history/runs.sqlite` and the workspace environment
layout remains `.omakure/envs/`. The release package is now one binary per
platform; it does not contain themes, widgets, or workspace scripts. Back up workspace state before upgrading: the run-state
migration and the legacy JSON cleanup are destructive, as follows.

### Destructive upgrade cleanups

Upgrading to this version of `omakure` triggers **two destructive
cleanups** on first launch against an existing workspace:

1. Every top-level `*.json` file in `<workspace>/.history/` is deleted
   (legacy per-run JSON history layout from pre-v0.1 releases).
2. If `<workspace>/.history/runs.sqlite` exists with the v0.1 schema
   (i.e. the `runs` table has no `state` column), the table is
   **dropped and recreated** with the new state-machine schema. Every
   row in the legacy table is lost.

Both cleanups are intentionally narrow:

- only top-level files in `history_dir()` are touched by the JSON cleanup
- only files whose extension is exactly `.json`
- subdirectories and `search-index.sqlite`
  are left untouched
- the schema rebuild only drops and recreates the `runs` and
  `run_traces` tables — not the database file or any other table

If you care about historical run data from older releases, **back up
`<workspace>/.history/` before upgrading**.
