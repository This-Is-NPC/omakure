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

## Unattended Signed-Bundle Enrollment

Targets generate `identity.key`, `transport.key`, and their certificates
locally on first start. Configure only authority public keys, the organization,
and SHA-256 bootstrap token/nonce hashes in `node.toml`; never place a token,
private key, or reusable credential in that file or an image.

After installation and node configuration, run the enrollment operation
explicitly. It consumes the signed `OMEB` bundle and a node-local, one-time
token file without echoing either value:

```text
omakure node enroll apply \
  --bundle-file /run/secrets/enrollment.bundle \
  --bootstrap-token-file /run/secrets/bootstrap.token \
   --bootstrap-nonce <one-time-lowercase-hex-nonce>
```

The token path must be absolute, owned by the node service principal, a
regular non-symlink file with private `0600` permissions (or the equivalent
Windows service ACL), and no larger than the configured token bound. A
successful activation removes the local token file; failed validation leaves
it untouched.

For the authenticated HTTP service, pass only the bundle and nonce to
`POST /v1/node/enrollment/bundle`; set `OMAKURE_BOOTSTRAP_TOKEN_FILE` or
`node serve --bootstrap-token-file` to the node-local token file. The route
requires `enrollment:write`, never returns the token, and writes only redacted
audit evidence.
