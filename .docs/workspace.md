# Workspace layout

Omakure treats the workspace as a filesystem score:

```
omakure-scripts/
├── .omaken/        # Curated flavors managed by Omakure
│   └── azure/
│       ├── index.lua   # Optional folder widget
│       ├── rg-list-all.bash
│       ├── rg-details.bash
│       └── rg-delete.bash
│   └── envs/       # Environment defaults (active file listed in .omaken/envs/active)
│       ├── active
│       └── env_template.conf
├── .history/       # Execution logs
└── omakure.toml    # Optional workspace config
```

If a folder includes `index.lua`, Omakure renders it in the TUI header panel. See `lua-widgets.md`.

Environment defaults live in `.omaken/envs/*.conf`. Use the TUI (Alt+E) to switch the active file.
Defaults are applied by matching field names (case-insensitive) to `key=value` pairs.
See `environments.md` for usage details.

The `.history/` folder stores local run logs and is ignored by git. Run
entries are keyed by the **absolute canonical path** of the executed
script so the same physical script always produces the same key,
regardless of which directory the run was launched from.

## Global workspace vs. session scripts root

Omakure tracks two distinct path anchors:

- **Global workspace** — owns `.history/`, `.omaken/`, `.omaken/envs/`,
  the SQLite search index, and `omakure.toml`. Resolved by the
  `scripts_dir()` precedence chain (`--scripts-dir` >
  `OMAKURE_SCRIPTS_DIR` > legacy env vars > debug `scripts/` fallback >
  `~/Documents/omakure-scripts`).
- **Scripts root** — the directory the TUI is currently browsing. By
  default this is the global workspace. When you launch the TUI with a
  positional path (`omakure .`, `omakure ../team-scripts`, `omakure
  /abs/path`), only the scripts root is overridden for that session.

Launching `omakure <PATH>` never creates `.omaken/`, `.history/`, or
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
