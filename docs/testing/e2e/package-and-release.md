# Package and Release

**Status:** CI and release workflow.

## Source

- `tests/packaging_smoke.rs`
- `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- `scripts/tasks/suite/package-release`, `scripts/tasks/atomic/package-artifact`,
  `scripts/tasks/atomic/binary-smoke`, `scripts/tasks/atomic/musl-static`, and
  `scripts/tasks/atomic/run-bounded`
- `scripts/release/package-release.sh`, `Dockerfile`, `.dockerignore`

## Run

Use the manifest-driven native suite and canonical release routes:

```bash
mise run test:integration
mise run package:release
scripts/tasks/atomic/check-all-targets
```

`package:release` owns the local build-to-archive interface. The release
workflow packages each matrix artifact after the selected platform suite; it
does not turn the packaging test into the platform test suite. Atomic commands
forward their remaining arguments, and long-running workflow gates use
`scripts/tasks/atomic/run-bounded` rather than open-ended command chains.

## Proves

- The Dockerfile is multi-stage, installs the expected headless runtime tools, and starts `node serve` on `0.0.0.0:7878` with explicit non-loopback allowance.
- `.dockerignore` excludes heavy repository paths.
- Compose and installers document loopback management HTTP, workspace/state volumes, fixed uid/gid, token-file auth, and service installation paths.
- Release matrices route Linux GNU/musl, Linux ARM64 GNU/musl, macOS Intel/ARM64, and Windows Intel/ARM64 through the four platform suites without naming collisions.
- `binary-smoke` owns release `--version` execution; `musl-static` owns static-link verification; archives contain exactly one root binary.
- Removed TUI dependencies/assets stay absent while vendored `mlua` remains for the `.lua` script kind.

## Does Not Prove

- These Rust packaging tests inspect file content and do not run `docker build`.
- CI ARM jobs are workflow evidence until the corresponding hosted jobs actually execute.
- Installer execution and service registration are not proven by these packaging checks; see [Fedora VM privilege](fedora-vm-privilege.md) for the separate manual evidence.

## Environment and Cleanup

Archive tests use temporary files and do not require a Docker daemon. The CI
`docker-smoke` job has a 20-minute job timeout: image build is capped at 10
minutes, container/volume setup and cleanup commands at 30-60 seconds, health
and readiness polling at 30 attempts with bounded curl calls, and the packaged
Cue and baseline flows use per-operation bounds of 60-180 seconds. Its
always-run cleanup removes the test container and named volumes, then fails if
any listed container, network, or volume remains.

Each CI/release platform matrix job has a 60-minute job bound. The selected
platform script forwards its target to native tests, release build,
`binary-smoke`, and (for musl) `musl-static`; the workflow package step handles
the per-target artifact, while `package:release` is the local
suite/package-release route and is not a substitute for matrix artifact
packaging.

`run-bounded` requires GNU `timeout` on Linux and enforces each supplied
per-operation bound with a kill-after margin. On macOS and Windows it uses
`gtimeout` when available; if it is unavailable, it emits this exact notice
before explicitly falling back to the platform job's 60-minute bound:

`atomic/run-bounded: GNU timeout unavailable; operation-level timeout is unavailable and caller/platform CI 60-minute bound applies`

This fallback must not be described as silent per-operation enforcement.

The Linux transport certification job has a 30-minute job timeout and wraps its
canonical gate in a 20-minute timeout with a 15-second kill-after margin. The
Linux Health Plane certification job has a 120-minute job timeout and bounds
the gate at 45 minutes and its induced-failure cleanup test at 60 minutes, each
with a 300-second kill-after margin. Their always-run cleanup checks are
documented in [Transport certification](transport-certification.md) and
[Health Plane Docker](health-plane-docker.md); the release workflow repeats
the transport certification and its resource inspection.

These hosted checks remove their own containers, networks, and volumes and fail
closed when resource inspection cannot run. Archive verification is bounded by
the 60-minute matrix job and by the invoked archive command.

## Troubleshooting

- Archive mismatch: list it with `tar -tzf` or the Windows ZIP equivalent and verify the binary name is the only root entry.
- Dockerfile contract failure: compare the asserted strings with the default command, not with a local override.
