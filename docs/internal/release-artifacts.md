# Release artifact format

The release workflow builds one headless `omakure` executable for each supported
target. It packages nothing else: no workspace scripts, no assets.

## Archives

| CI platform | Rust target | Release asset |
|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `omakure-vX.Y.Z-linux-x86_64.tar.gz` |
| Linux x86_64, musl | `x86_64-unknown-linux-musl` | `omakure-vX.Y.Z-linux-musl-x86_64.tar.gz` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` | `omakure-vX.Y.Z-linux-aarch64.tar.gz` |
| Linux aarch64, musl | `aarch64-unknown-linux-musl` | `omakure-vX.Y.Z-linux-musl-aarch64.tar.gz` |
| macOS x86_64 | `x86_64-apple-darwin` | `omakure-vX.Y.Z-darwin-x86_64.tar.gz` |
| macOS aarch64 | `aarch64-apple-darwin` | `omakure-vX.Y.Z-darwin-aarch64.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `omakure-vX.Y.Z-windows-x86_64.zip` |
| Windows aarch64 | `aarch64-pc-windows-msvc` | `omakure-vX.Y.Z-windows-aarch64.zip` |

`scripts/install/install.sh` selects the host architecture and prefers the matching `linux-musl`
archive, falling back to the matching glibc archive when a release predates it.
The PowerShell installer selects the matching Windows `x86_64` or `aarch64`
archive. Omakure is installed on machines the operator does not control, and a
glibc-linked binary refuses to start on any distribution older than the machine
that built it, so the portable build is the default and the dynamic one is kept
for anyone who wants the system allocator and resolver.

Each archive contains exactly one root entry:

- `omakure` on Linux/macOS
- `omakure.exe` on Windows

## CI and release reuse

The CI and release workflows share the same matrix-facing route:

```text
matrix platform + target
  -> scripts/tasks/check/platform/{linux-gnu,linux-musl,macos,windows}
  -> native tests, target build, static-link check where applicable, binary smoke
  -> archive packaging and binary-only assertion
```

Each release matrix entry invokes the selected platform script rather than
embedding test/build/static-link/smoke commands in workflow YAML. The release
workflow then packages the resulting binary and asserts the archive contents.
The eight entries cover the targets above, and each platform/release matrix job
has a 60-minute bound. Atomic routes forward their remaining arguments.
`run-bounded` requires GNU `timeout` on Linux and enforces per-operation
ceilings with a kill-after margin. On macOS and Windows it uses `gtimeout` when
available; without it, the atomic explicitly falls back to the platform job's
60-minute bound. These jobs are evidence for those target builds and package
contents, not proof that every binary ran on every other platform.

Runner prerequisites are platform-specific: musl entries need `musl-tools`
and `musl-gcc`; macOS entries need an owned physical `RUNNER_TEMP`; and
Windows entries need the supported MSVC target and static CRT setup. Linux is
the only host on which the complete local `check:full` scope is available.
Destructive Fedora VM/KVM certification is manual and excluded from release
and pre-push gates.

`tests/packaging_smoke.rs` verifies the source/package contract without
requiring Docker. The Linux Docker smoke and certification jobs are separate
evidence; they do not turn the packaging test into a local service or installer
test.

For local release preparation, use the same canonical routes used by the
automation:

```bash
mise run build:release
mise run package:release
scripts/tasks/check/platform/linux-gnu
```

The first two commands build and package the local host artifact; the
zero-argument Linux GNU route uses the host target's unqualified
`target/release/omakure` path. Explicit target-qualified routes are selected by
CI/release matrix metadata; use the corresponding `linux-musl`, `macos`, or
`windows` suite only on a runner that provides its documented prerequisites.

The runtime workspace is created or mounted separately. See `../deployment.md`
for the node-service container and `../workspace.md` for its volume layout.
