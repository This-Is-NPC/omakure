# Environment documents

Environment defaults live in `.omakure/envs/*.conf`. The active file name is stored in `.omakure/envs/active`.

## How it works

- Each line is `KEY=value`.
- Keys are matched (case-insensitive) to schema field names.
- When a match exists, the value is used as the default in the TUI.

## Runtime injection into scripts

Beyond prefilling TUI field defaults, the **active** env file is injected
into the environment of the spawned script process (its `os.environ`) at
every run entry point — the TUI, the queue worker, and `omakure run`. So
`.omakure/envs/<active>.conf` is where you put secrets and config that a
script reads from the environment at runtime.

- **Key case is preserved** for injected vars (unlike the TUI-prefill
  match, which is case-insensitive). `PATH` stays `PATH` — it is not
  lowercased — so case-sensitive vars like `PATH`/`VIRTUAL_ENV` work on
  Linux/macOS.
- **Variable expansion**: values support single-pass `$VAR` and
  `${VAR}` substitution sourced from the merged env. Undefined vars
  expand to empty; `\$` is a literal `$`. No command substitution
  (`$(...)`/backticks) and no recursion. See
  `env-injection-spec.md` §2 for the full grammar.
- **Precedence** (lowest → highest): parent shell env < managed active
  env (`.omakure/envs/*.conf`) < CLI `--env-file` < Omakure-reserved
  (`OMAKURE_RUN_ID`, `OMAKURE_SCRIPTS_DIR` always win and cannot be
  overridden).
- **Secrets are not persisted**: injected values reach the child process
  at spawn only. They are never written to `runs.sqlite`, logs, or the
  trace. (Residual exposure: readable via `/proc/<pid>/environ` and
  inherited by grandchild processes — an accepted tradeoff.) See
  `env-injection-spec.md` §3.

## Selecting an interpreter / virtual env (venv-via-PATH)

Interpreter selection is just env injection — there is no separate
`python=` setting. Prepend a virtual-env `bin` directory to `PATH` (and
optionally set `VIRTUAL_ENV`) in the active env file, and Omakure runs
that interpreter. The interpreter is resolved to an **absolute path** by
a which-style lookup against the injected `PATH`, so the venv's `python`
runs instead of the system one. The same mechanism is language-agnostic
— it works for Node (`nvm`, `node_modules/.bin`), Ruby (`rbenv`), etc.

```
VIRTUAL_ENV=/home/me/project/.venv
PATH=/home/me/project/.venv/bin:$PATH
```

Run `omakure config` (or `omakure env`) to see the resolved active-env
keys (sensitive values masked) and the absolute interpreter path Omakure
will actually execute — useful for debugging env collisions.

## Switch environments

Use the TUI (`Ctrl+/` then `e`) to select the active file.

## Environments UI

The Environments screen shows a preview panel on the right for the selected file.
The preview lists parsed KEY=VALUE entries and masks sensitive values with `***`.

Preview scroll shortcuts:

- `PgUp` / `PgDn`
- `Home` / `End`

## Example

```
SUBSCRIPTION_ID=00000000-0000-0000-0000-000000000000
RESOURCE_GROUP=rg-prod
REGION=eastus
```

Preview example:

```
SUBSCRIPTION_ID=***
RESOURCE_GROUP=rg-prod
REGION=eastus
```

## Start from the template

Copy `.omakure/envs/env_template.conf` to a new `.conf` file and edit the values.

## Per-directory session env (`omakure.conf`)

When you launch the TUI with a positional path (`omakure .`,
`omakure ../team-scripts`, etc.) and the target directory contains an
`omakure.conf` file at its root, that file becomes the **session-active
environment** for the duration of the TUI session. It uses the same
`KEY=value` format as `.omakure/envs/*.conf` and is parsed with the same
parser. Schema field defaults are populated from it exactly as they
would be from a globally active env file.

The session env override is **read-only**:

- It is never copied into `.omakure/envs/`.
- It never updates `.omakure/envs/active`.
- It never creates or deletes any file inside the scripts root.
- The Environments screen (`Ctrl+/` then `e`) keeps showing the global
  `.omakure/envs/` list. `omakure.conf` is not listed there and cannot be
  edited from the TUI.
- The session override always wins for the duration of the session, so
  activating or deactivating a global env via the Environments screen
  while a session `omakure.conf` is in effect will update the global
  `.omakure/envs/active` file but the defaults shown for schema fields
  continue to come from `omakure.conf`. The change to the global active
  file takes effect on the next plain `omakure` invocation.

If `omakure.conf` is **absent**, the session falls back to the file
currently pointed to by `.omakure/envs/active` in the global workspace
— exactly the same behavior as launching `omakure` with no positional
path. If neither is present, no environment defaults are applied.

The parser is the same lenient one used for `.omakure/envs/*.conf`:
blank lines, comments (`#`, `;`), and lines without an `=` sign are
silently skipped, so a `omakure.conf` containing partially malformed
content still applies the valid `KEY=value` pairs as defaults. If the
file is **unreadable** (e.g. permission denied), Omakure surfaces the
I/O error through the existing environment error reporting path and
falls back to the globally active env. The TUI still launches in
either case.

When the scripts root coincides with the global workspace (i.e. plain
`omakure` with no positional path), `<global-workspace>/omakure.conf`
is **not** interpreted as a session override. The session-env feature
only activates when a positional path was supplied on the command line.
