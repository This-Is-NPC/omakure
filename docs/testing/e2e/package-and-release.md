# Package and Release

**Status:** CI and release workflow.

## Source

- `tests/packaging_smoke.rs`
- `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- `.github/package-release.sh`, `Dockerfile`, `.dockerignore`

## Run

```bash
cargo test --test packaging_smoke --locked
cargo test --test packaging_smoke --locked -- release_tarball_contains_only_the_required_binary
```

## Proves

- The Dockerfile is multi-stage, installs the expected headless runtime tools, and starts `node serve` on `0.0.0.0:7878` with explicit non-loopback allowance.
- `.dockerignore` excludes heavy repository paths.
- Compose and installers document loopback management HTTP, workspace/state volumes, fixed uid/gid, token-file auth, and service installation paths.
- Release matrices build Linux GNU/musl, Linux ARM64 GNU/musl, macOS Intel/ARM64, and Windows Intel/ARM64 assets without naming collisions.
- musl artifacts are checked for static linkage; archives contain exactly one root binary.
- Removed TUI dependencies/assets stay absent while vendored `mlua` remains for the `.lua` script kind.

## Does Not Prove

- These Rust packaging tests inspect file content and do not run `docker build`.
- CI ARM jobs are workflow evidence until the corresponding hosted jobs actually execute.
- Installer execution and service registration are not proven by these packaging checks; see [Fedora VM privilege](fedora-vm-privilege.md) for the separate manual evidence.
- The release build matrix has no job-level timeout in `.github/workflows/release.yml`; only the release publication job has a 15-minute job timeout.

## Environment and Cleanup

Archive tests use temporary files and do not require a Docker daemon. The CI
`docker-smoke` job has a 20-minute job timeout: image build is capped at 10
minutes, container/volume setup and cleanup commands at 30-60 seconds, health
and readiness polling at 30 attempts with bounded curl calls, and the packaged
Cue and baseline flows use per-operation bounds of 60-180 seconds. Its
always-run cleanup removes the test container and named volumes, then fails if
any listed container, network, or volume remains.

The Linux transport certification job has a 30-minute job timeout and wraps its
canonical gate in a 20-minute timeout with a 15-second kill-after margin. The
Linux Health Plane certification job has a 120-minute job timeout and bounds
the gate at 45 minutes and its induced-failure cleanup test at 60 minutes, each
with a 300-second kill-after margin. Their always-run cleanup checks are
documented in [Transport certification](transport-certification.md) and
[Health Plane Docker](health-plane-docker.md); the release workflow repeats
the transport certification and its resource inspection.

These hosted checks remove their own containers, networks, and volumes and
fail closed when resource inspection cannot run. The release build's archive
verification is bounded by the workflow step only where the runner or command
provides a bound; this page does not infer an unlisted timeout.

## Troubleshooting

- Archive mismatch: list it with `tar -tzf` or the Windows ZIP equivalent and verify the binary name is the only root entry.
- Dockerfile contract failure: compare the asserted strings with the default command, not with a local override.
