# Environment documents

Environment defaults live in `.omaken/envs/*.conf`. The active file name is stored in `.omaken/envs/active`.

## How it works

- Each line is `KEY=value`.
- Keys are matched (case-insensitive) to schema field names.
- When a match exists, the value is used as the default in the TUI.

## Switch environments

Use the TUI (Alt+E) to select the active file.

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

Copy `.omaken/envs/env_template.conf` to a new `.conf` file and edit the values.

## Per-directory session env (`omakure.conf`)

When you launch the TUI with a positional path (`omakure .`,
`omakure ../team-scripts`, etc.) and the target directory contains an
`omakure.conf` file at its root, that file becomes the **session-active
environment** for the duration of the TUI session. It uses the same
`KEY=value` format as `.omaken/envs/*.conf` and is parsed with the same
parser. Schema field defaults are populated from it exactly as they
would be from a globally active env file.

The session env override is **read-only**:

- It is never copied into `.omaken/envs/`.
- It never updates `.omaken/envs/active`.
- It never creates or deletes any file inside the scripts root.
- The Environments screen (`Alt+E`) keeps showing the global
  `.omaken/envs/` list. `omakure.conf` is not listed there and cannot be
  edited from the TUI.
- The session override always wins for the duration of the session, so
  activating or deactivating a global env via the Environments screen
  while a session `omakure.conf` is in effect will update the global
  `.omaken/envs/active` file but the defaults shown for schema fields
  continue to come from `omakure.conf`. The change to the global active
  file takes effect on the next plain `omakure` invocation.

If `omakure.conf` is **absent**, the session falls back to the file
currently pointed to by `.omaken/envs/active` in the global workspace
— exactly the same behavior as launching `omakure` with no positional
path. If neither is present, no environment defaults are applied.

The parser is the same lenient one used for `.omaken/envs/*.conf`:
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
