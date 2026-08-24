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
headless `node serve` command when an HTTP endpoint and background loops are needed.

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

## Machine Node Service

Normal installer use is per-user and never provisions a privileged service.
Machine-service setup is an explicit opt-in and requires an existing secure
tokens TOML containing Argon2id hashes. The installer does not generate or
print a service token:

```bash
sudo bash install.sh --install-node-service \
  --node-tokens-file /secure/path/omakure_tokens.toml
```

Linux registers `omakure-node.service` for the `omakure` system principal.
macOS registers `com.omakure.node` for `_omakure`. Windows PowerShell registers
`OmakureNode` as `NT AUTHORITY\LocalService`; its node state, configuration, and
tokens DACLs contain only `SYSTEM` and `LocalService`. Default machine paths are
`/etc/omakure/node.toml` and `/var/lib/omakure` on Linux, the corresponding
`/Library/Application Support/Omakure` paths on macOS, and
`%ProgramData%\Omakure` on Windows. The workspace is kept outside node state.

Reinstalling updates the binary and service definition but preserves
`node.toml`, `identity.key`, `node.sqlite`, and registration. Remove only the
service with `--uninstall-node-service`; deleting node state additionally
requires `--uninstall-node-state --confirmed` (or the matching PowerShell
switches). Platform service registration and permission checks are covered by
source/static packaging tests and hosted CI; local validation here does not
claim macOS or Windows execution.
