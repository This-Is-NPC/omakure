# Scripts path

By default, Omakure reads scripts from:

- `~/Documents/omakure-scripts` (Linux/macOS)
- `%USERPROFILE%\Documents\omakure-scripts` (Windows)

## Resolution precedence

The scripts directory is resolved in this order (first match wins):

1. `--scripts-dir <PATH>` CLI flag.
2. `OMAKURE_SCRIPTS_DIR` environment variable.
3. Legacy `OVERTURE_SCRIPTS_DIR` (accepted for backward compatibility).
4. Legacy `CLOUD_MGMT_SCRIPTS_DIR` (accepted for backward compatibility).
5. The repo `scripts/` folder **in debug builds only** (so `cargo run` uses it automatically during development).
6. `~/Documents/omakure-scripts` if it exists.
7. Legacy `~/Documents/overture-scripts` / `~/Documents/cloud-mgmt-scripts` if they exist.
8. Fallback: `~/Documents/omakure-scripts` (created on first launch).

The Windows Documents path is resolved via the registry, so a
relocated user-profile Documents folder is honored.

## Change the default path

Set `OMAKURE_SCRIPTS_DIR` before running `omakure`.

Linux/macOS:

```bash
export OMAKURE_SCRIPTS_DIR=/path/to/scripts
omakure
```

Windows (PowerShell):

```powershell
$env:OMAKURE_SCRIPTS_DIR = "C:\path\to\scripts"
omakure
```

## Open the TUI against any directory (session-only)

If you want to point the TUI at an ad-hoc directory without changing
the global workspace, pass the directory as a positional argument:

```bash
omakure .                     # current directory
omakure ../team-scripts       # relative path
omakure /abs/path/to/scripts  # absolute path
```

Unlike `OMAKURE_SCRIPTS_DIR` and `--scripts-dir`, the positional path
is a **session-only scripts root**: history, environments, the search
index, and `omakure.toml` always stay in the global workspace and are
never created inside the target directory. The positional path is
mutually exclusive with `--scripts-dir`.

See `usage.md` for the full description of the global-vs-session split
and `environments.md` for the optional `<PATH>/omakure.conf` session
env.

## Exclude paths from scanning

Create a `.omakureignore` file at the scripts root, or inside any child
directory, to keep matching files and directories out of Omakure's script
scan. Ignored scripts do not appear in the TUI, `omakure scripts`, the
search index, or the scheduler because all of those surfaces use the same
recursive scanner.

Example:

```gitignore
# Helpers and generated files
helpers/
fixtures/*.sh
*.tmp.py
scratch.py
```

Supported pattern subset:

- Blank lines and lines starting with `#` are ignored.
- Patterns are evaluated relative to the directory containing that
  `.omakureignore` file. When multiple files apply while Omakure descends a
  tree, parent rules and child rules are both active.
- A leading `/` anchors to the directory containing that `.omakureignore`
  (`/scratch.py` in `scripts/.omakureignore` matches `scripts/scratch.py`,
  not `other/scratch.py`).
- A trailing `/` means directory-only and prunes the whole subtree, for
  example `helpers/` skips `helpers` and everything below it.
- `*` matches any sequence of characters.
- Patterns containing `/` match paths relative to the directory containing
  that `.omakureignore`, for example `fixtures/*.sh`.
- Patterns without `/` match any path component, for example `scratch.py`
  or `*.tmp.py` at any depth.

Nested `.omakureignore` files are supported. Unsupported gitignore features
are not implemented: negation with `!`, `**` special semantics, character
classes, and escaped `#` comments. If `.omakureignore` cannot be read or
decoded, Omakure prints a warning and continues scanning with the remaining
ignore rules plus the built-in skips (`.history`, `.git`, and `.omaken/envs`).

## Development note

In debug builds, the app will use the repo `scripts/` folder if it exists. You can still override it with `OMAKURE_SCRIPTS_DIR`.
