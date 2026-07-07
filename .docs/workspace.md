# Workspace layout

Omakure treats the workspace as a filesystem store:

```
omakure-scripts/
├── .omakure/              # Omakure-owned runtime metadata
│   ├── envs/              # Environment defaults (active file listed in .omakure/envs/active)
│   │   ├── active
│   │   └── env_template.conf
│   ├── batteries.json     # Registered Battery sources
│   ├── batteries/
│   │   ├── cache/         # Untrusted synced Battery Git checkouts
│   │   └── installed/     # Provenance records for installed Battery scripts
│   ├── daemon.pid         # `omakure serve` PID lock (present when the scheduler is running)
│   └── daemon.log         # Structured scheduler log (RFC3339 lines)
├── .history/              # Runtime state (SQLite)
│   ├── runs.sqlite            # Run state machine + structured traces
│   └── search-index.sqlite    # Script search index
└── omakure.toml           # Optional workspace config
```

If a folder includes `index.lua`, Omakure renders it in the TUI header panel. See `lua-widgets.md`.

Environment defaults live in `.omakure/envs/*.conf`. Use the TUI (`Ctrl+/` then `e`) to switch the active file.
Defaults are applied by matching field names (case-insensitive) to `key=value` pairs.
See `environments.md` for usage details.

Batteries store their registry and cache under `.omakure/`. Cached Battery
repositories are untrusted and are not executed directly; `battery install`
copies validated scripts into the scripts workspace and records provenance. See
`batteries.md`.

The `.history/` folder stores local run logs and is ignored by git. Run
entries are keyed by the **absolute canonical path** of the executed
script so the same physical script always produces the same key,
regardless of which directory the run was launched from.

## Global workspace vs. session scripts root

Omakure tracks two distinct path anchors:

- **Global workspace** — owns `.history/`, `.omakure/`, `.omakure/envs/`,
  the SQLite search index, and `omakure.toml`. Resolved by the
  `scripts_dir()` precedence chain (`--scripts-dir` >
  `OMAKURE_SCRIPTS_DIR` > legacy env vars > debug `scripts/` fallback >
  `~/Documents/omakure-scripts`).
- **Scripts root** — the directory the TUI is currently browsing. By
  default this is the global workspace. When you launch the TUI with a
  positional path (`omakure .`, `omakure ../team-scripts`, `omakure
  /abs/path`), only the scripts root is overridden for that session.

Launching `omakure <PATH>` never creates `.omakure/`, `.history/`, or
`omakure.toml` inside `<PATH>`. The `Workspace` type has an internal
invariant that `ensure_layout()` is strictly bound to the global root,
so the positional target stays untouched.

History entries recorded from a session-override TUI are written to the
**global** `.history/` directory and use the absolute canonical script
path. The in-session history view filters entries to those whose
absolute path lies within the active scripts root, so:

- Plain `omakure` shows every entry whose script lives under the global
  workspace, including runs originally launched from `omakure <PATH>`
  sessions whose target was a subdirectory of the global workspace.
- `omakure <PATH>` shows entries whose script lives under `<PATH>`,
  regardless of which session originally recorded them.

Legacy history entries (which stored workspace-relative paths) continue
to load and display; the filter resolves them against the global
workspace root so they remain visible in the plain-`omakure` case.

## Removed Omaken layout

The old Omaken flavor concept and `.omaken/` directory are removed from the
active workspace contract. Omakure does not read, create, or migrate
`.omaken/`; reusable script repositories are managed by Batteries under
`.omakure/`.
