# Scripts path

By default, Omakure reads scripts from:

- `~/Documents/omakure-scripts` (Linux/macOS)
- `%USERPROFILE%\Documents\omakure-scripts` (Windows)

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

## Development note

In debug builds, the app will use the repo `scripts/` folder if it exists. You can still override it with `OMAKURE_SCRIPTS_DIR`.
