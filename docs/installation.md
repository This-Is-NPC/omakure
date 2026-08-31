# Installation

## Release installation

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/scripts/install/install.sh \
  | bash -s -- --repo This-Is-NPC/omakure
```

Windows PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/This-Is-NPC/omakure/main/scripts/install/install.ps1 | iex"
```

Verify an installation with named commands:

```bash
omakure --version
omakure --help
omakure doctor
```

The first workspace operation creates the selected workspace, `.omakure/`,
`.history/`, and `omakure.toml`. Release and source installers install the
binary only and never copy repository automation into the workspace. The
binary itself is a CLI; deploy the headless `node serve` command when an HTTP
endpoint and background loops are needed. External Battery repositories own
subject scripts; register, sync, and explicitly install them with the Battery
commands.

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
bash scripts/install/install-from-source.sh
```

For development, use `cargo build`, `cargo test`, `mise run lint`, and
`mise run dev:smoke`; see `internal/development.md`.

## Machine Node Service

Normal installer use is per-user and never provisions a privileged service.
Machine-service setup is an explicit opt-in and requires an existing secure
tokens TOML containing Argon2id hashes. The installer does not generate or
print a service token:

```bash
sudo bash scripts/install/install.sh --install-node-service \
  --node-tokens-file /secure/path/omakure_tokens.toml
```

Linux registers `omakure-node.service` for the `omakure` system principal.
macOS registers `com.omakure.node` for `_omakure`. Windows PowerShell registers
`OmakureNode` as `NT AUTHORITY\LocalService`; its node state, configuration, and
tokens DACLs contain only `SYSTEM` and `LocalService`. Default machine paths are
`/etc/omakure/node.toml` and `/var/lib/omakure` on Linux, the corresponding
`/Library/Application Support/Omakure` paths on macOS, and
`%ProgramData%\Omakure` on Windows. The workspace is kept outside node state.

These are restricted service identities, not an administrative execution
broker. Omakure has no built-in `sudo`, privilege elevation, or per-Battery OS
permission grant. An operation that changes machine configuration needs the
deployment to grant the service account the narrow host permissions that
operation requires; the official Linux unit additionally sets
`NoNewPrivileges=true`.

Reinstalling updates the binary and service definition but preserves
`node.toml`, `identity.key`, `node.sqlite`, and registration. Remove only the
service with `--uninstall-node-service`; deleting node state additionally
requires `--uninstall-node-state --confirmed` (or the matching PowerShell
switches).

### What is proven about the service, and what is not

`tests/packaging_smoke.rs` reads the installer sources and asserts that the
systemd unit, the launchd plist, and the `sc.exe` registration are written the
way this page describes, with the node-state paths and ACLs it names. That is a
static check of what the installers *say*.

The default Rust test suite executes only the Unix uninstall path:
`scripts/install/install.sh --uninstall-node-service` runs with `systemctl` replaced by a shim
that records what it was asked and does nothing. This is mocked Unix coverage,
not a native service uninstall. `tests/packaging_smoke.rs` statically inspects
the Unix installer, the macOS launchd text, and the Windows installer sources;
it does not execute a native macOS or Windows installer. No test anywhere runs
the install path, registers a real service, or starts one. The Docker gates start
`node serve` as a container entrypoint, which is not a service manager starting
it at boot, and the macOS and Windows CI jobs run the test suite without
touching either installer. The separate local certification described below is
the Linux exception; it is not part of CI because it requires KVM and writable
system libvirt.

Outside the test suite, the Linux install path has been executed on two real
Fedora virtual machines: `scripts/install/install.sh` created the unprivileged `omakure` service
account, wrote the unit, and systemd started `node serve` under it, and the pair
was then driven through direct transport, an authorized Cue, baseline
push/drift/rollback, and revocation. Restart was exercised, and two defects
reachable only through the installed service were found that way — the tokens
file losing its `root:omakure 0640` ownership on `token generate --append`, and
the service account being homed inside the 0700 state directory. A cold boot
into a running node was not separately recorded; start, restart, and running as
the service account were. `install.ps1` is never executed on any platform, and
the launchd path has never been run either.

Treat the first machine-service install on a new host as unverified and confirm
it yourself:

```bash
systemctl status omakure-node            # Linux
launchctl print system/com.omakure.node  # macOS
sc.exe query OmakureNode                 # Windows
```

Getting real evidence needs a disposable machine — `systemd-nspawn --boot`,
a VM, or a CI runner discarded after the run. A privileged container sharing
the host's cgroup or PID namespace is not a substitute: its init does not see
itself as isolated and will signal host processes it judges orphaned.

### Fedora VM privilege certification

`mise run cert:vm` is the bounded, destructive local gate
for Linux privilege delegation. It builds the current binary, verifies a pinned
Fedora Cloud image by SHA-256, then boots a corporate Conductor, an intentionally
broad root runner, and a primary Performer on `qemu:///system`. The comparison
runner exposes the authenticated Omakure API and queue worker as root. It does
not repurpose `omakure-node.service`: production node state is deliberately
tied to the `omakure` system principal and rejects a root-owned replacement.
The primary Performer keeps the shipped `User=omakure` and
`NoNewPrivileges=true` settings and grants that principal only `start` on
`omakure-certified-root-operation.service` through a fixed Polkit rule.

The Conductor sends an authenticated queue request to the root comparison and a
Battery-backed Remote Cue to the unprivileged Performer. The gate checks the
real root effects, run history, returned Signal for the Cue, and exactly one
observed effect per authorized request. It also checks undeclared-script
rejection on the Performer, employee isolation, arbitrary-unit denial, and
revocation. The `employee` guest account is not in
`wheel`, cannot use passwordless sudo, cannot read tokens or node identity,
cannot modify the workspace or policy, and cannot call a management API without
a token.

The host prerequisites are Linux with readable and writable `/dev/kvm`, the
checkout's `scripts/fixtures/fedora-vm-privilege/` directory, and these commands: `cargo`, `curl`,
`jq`, `scp`, `sha256sum`, `ssh`, `ssh-keygen`, `stat`, `tar`, `timeout`,
`virsh`, `virt-install`, and `xorriso`. Libvirt must be reachable at
`qemu:///system` (by default), with an active `default` network, a running
`images` storage pool, and permission to create, upload, destroy, and inspect
domains and volumes there.

The default base volume is `omakure-fedora-44-1.5-base.qcow2`, downloaded from
`https://download.fedoraproject.org/pub/fedora/linux/releases/test/44_Beta/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44_Beta-1.5.x86_64.qcow2`
and checked against SHA-256
`28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f` before it
is cached or uploaded. Override the URI, network, pool, base volume, image URL,
checksum, or run project with `OMAKURE_VM_LIBVIRT_URI`,
`OMAKURE_VM_NETWORK`, `OMAKURE_VM_STORAGE_POOL`, `OMAKURE_VM_BASE_VOLUME`,
`OMAKURE_VM_IMAGE_URL`, `OMAKURE_VM_IMAGE_SHA256`, and
`OMAKURE_VM_CERTIFICATION_PROJECT`, respectively. The checksum override must
be a lowercase SHA-256 value. The verified base volume is retained as a cache.
Every run-specific domain, overlay, seed ISO, temporary key, and token is
removed by an exit trap. Cleanup verification fails closed: if domain or
volume inspection errors, the certification fails rather than treating the
resources as absent. Run `mise run cert:vm-cleanup` to induce a failure after
the first VM is created and independently verify that cleanup.

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
