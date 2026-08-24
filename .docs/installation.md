# Installation

## Release installation

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.sh \
  | bash -s -- --repo This-Is-NPC/omakure
```

Windows PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command \
  "irm https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.ps1 | iex"
```

Verify an installation with named commands:

```bash
omakure --version
omakure --help
omakure --json doctor
```

The first workspace operation creates the selected workspace, `.omakure/`,
`.history/`, and `omakure.toml`. The binary itself is a CLI; deploy the
headless `engine` command when an HTTP endpoint and background loops are needed.

## Update

```bash
omakure update
omakure update --version vX.Y.Z --repo This-Is-NPC/omakure
```

Linux/macOS update needs `curl` or `wget` and `tar`; Windows uses PowerShell.
Existing workspace scripts are not overwritten by the update flow.

## Uninstall

```bash
omakure uninstall
omakure uninstall --scripts  # also permanently deletes the workspace
```

Back up `.history/` and `.omakure/` before the destructive form.

## Install from source

```bash
bash install-from-source.sh
```

For development, use `cargo build`, `cargo test`, `mise run lint`, and
`mise run dev`; see `development.md`.
