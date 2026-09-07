# Workspace layout

Omakure treats one directory as both the scripts root and the owner of its
runtime metadata. The root is selected with `--scripts-dir` or the documented
environment/default precedence.

```text
omakure-scripts/
├── .omakure/
│   ├── envs/
│   │   ├── active
│   │   └── env_template.conf
│   ├── batteries.json
│   ├── batteries/
│   │   ├── cache/             # untrusted synced repositories
│   │   └── installed/         # install provenance
│   ├── daemon.pid             # standalone scheduler lock
│   └── daemon.log             # standalone scheduler log
├── .history/
│   ├── runs.sqlite             # runs, queue, and traces
│   └── search-index.sqlite     # script search index
├── omakure.toml                # workspace configuration
└── <scripts and folders>
```

`ensure_layout` creates metadata only below the selected workspace. The CLI has
no session-root or positional-path mode, so there is no second path anchor and
no per-directory `omakure.conf` contract.

Environment files use `KEY=value` lines and are managed through `omakure env`
or the authenticated HTTP environment routes. Their values can be injected
into child processes but sensitive values are masked in diagnostics and are not
persisted in runs, logs, or traces.

The `.history/` directory is private Omakure state. Use `history`, `queue`, and
`trace` commands or their HTTP equivalents instead of opening SQLite directly.
Run rows use canonical script paths and include state-machine, actor, trigger,
timing, output, and provenance fields.

The run database uses a 2-second SQLite busy timeout. The separate
`search-index.sqlite` database uses a 500-millisecond busy timeout. Both are
workspace-local files and are not supported as shared network storage.

## Executable subject boundary

Direct runs and queued runs accept only files inside the selected workspace.
Components named `.omakure`, `.history`, or `.git` are reserved (case-insensitive),
including when nested below a subject folder. Both the requested path and its
canonical destination are checked, so a symlink alias cannot turn Battery cache
into an installed subject. Syncing a Battery alone does not authorize running
its cached scripts; install the desired subject first.

The shared executor repeats this check before resolving secrets or launching
any direct, queued, or scheduled run. Rows queued before this rule, or whose
script was replaced with a metadata symlink, fail without launching a child.
Regular subject symlinks within the workspace remain supported. This is not a
sandbox against a local user who can concurrently rewrite workspace files;
restrict filesystem write access to trusted users.

## Ignore rules

`.omakureignore` files can be placed at the workspace root or below it. Rules
apply relative to their file, support directory-only patterns, and combine
parent and child rules. The supported subset is documented in
`scripts-path.md`; ignored content is excluded consistently from CLI listing,
search, HTTP tree routes, and scheduling.

## Single-host storage

SQLite is local runtime state, not a distributed queue. Run one writer topology
per workspace volume, keep API/workers/node service on the same host, and do not use
NFS/CIFS or scale replicas over one `.history` directory.
