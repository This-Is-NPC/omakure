# omakure

Rust TUI to navigate a curated automation workspace, render optional folder widgets, and run
scripts that describe their parameters with a JSON schema. You organize folders, Omakure builds
the navigation, and the engine collects required values in a guided flow with execution history.

## Requirements

- Rust toolchain (development only)
- Git (Windows users: install Git for Windows so `git` and `bash` are on PATH)
- Bash (for `.bash`/`.sh` scripts)
- PowerShell (optional, for `.ps1` scripts)
- Python (optional, for `.py` scripts)
- `jq`

### Windows/macOS notes

- Windows: use Git for Windows (Git Bash) or WSL; ensure `git`, `bash`, and `jq` are in PATH.
- macOS: install `bash` and `jq` with Homebrew if missing.
- Scripts must use LF line endings (CRLF can break bash).
- Prefer Windows Terminal/PowerShell or Git Bash; CMD may not render the TUI well.
- Quote paths with spaces in scripts (e.g. `"C:\\Users\\Name\\Documents"`).

## Quick start

1) Install from releases:

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.sh | bash -s -- --repo This-Is-NPC/omakure
```

Windows (PowerShell):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.ps1 | iex"
```

2) Run:

```bash
omakure
```

3) Put scripts under `~/Documents/omakure-scripts` (Windows: `%USERPROFILE%\Documents\omakure-scripts`). Omakure scans this tree (including `.omaken`) for `.bash`, `.sh`, `.ps1`, and `.py` scripts.

4) Make the script visible to Omakure by exposing a schema: when `SCHEMA_MODE=1`, print JSON with `Name`, `Description`, and `Fields`. The `omakure init my-script` command generates a template with the schema block.

## Advanced

- Change the default scripts path: `.docs/scripts-path.md`
- Environment documents and defaults: `.docs/environments.md`

## Documentation

- Installation, updates, and uninstall: `.docs/installation.md`
- Workspace layout and defaults: `.docs/workspace.md`
- Scripts path overrides: `.docs/scripts-path.md`
- Environment documents: `.docs/environments.md`
- CLI usage: `.docs/usage.md`
- How to create a script: `.docs/how-to-create-a-script.md`
- How it works (overview + examples): `.docs/how-it-works.md`
- Development guide: `.docs/development.md`
- Release artifacts: `.docs/release-artifacts.md`

## License

AGPL-3.0-only. See `LICENSE`.
