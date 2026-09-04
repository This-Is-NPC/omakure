# Development

Omakure is built and exercised as a headless CLI/HTTP application. Do not use
bare `omakure` as an application launch command: no-argument invocation only
prints help and returns; development entry points must name a command.

## Checks and hooks

Install the tracked repository hooks from the repository root:

```bash
mise run hooks:install
```

This sets local Git `core.hooksPath` to `.githooks` and leaves unrelated global
Git configuration untouched. The hooks are exact thin wrappers:

| Hook | Canonical script | Scope |
|---|---|---|
| pre-commit | `scripts/tasks/check/fast` | Shared static/fixture checks, formatting, Clippy, and the library test atomic |
| pre-push | `scripts/tasks/check/full` | The shared checks once, then the complete locally executable Linux gate |

Run the gates directly or through their direct Mise routes:

```bash
scripts/tasks/check/fast
scripts/tasks/check/full
mise run check:fast
mise run check:full
```

An atomic under `scripts/tasks/atomic/` performs one operation. A suite under
`scripts/tasks/suite/` aggregates atomics or retained certification scripts.
The four platform suites under `scripts/tasks/check/platform/` are
`linux-gnu`, `linux-musl`, `macos`, and `windows`; each validates its target
runner and delegates tests/builds/smoke to the canonical atomics and suites.
Neither check gate duplicates the other.

Fast is intentionally limited to shell/YAML/static contract fixtures,
formatting, Clippy, and the library tests. Full adds all-target native tests,
development smoke, Usage and operation-catalog checks, deterministic
coverage, complexity calibration/ratchet/audit, release packaging, VM policy
inspection, Docker smoke, transport and Health certification, and cleanup
verification. Full requires Linux, Docker Engine and Compose, `jq`, SQLite,
Python with PyYAML, GNU `timeout`, and the pinned Rust toolchain. A local Linux
host is the only host with the complete full scope.

Hosted CI and release reuse the same call graph:

```text
hook -> check/{fast,full} -> atomic/suite
mise task -> one canonical atomic, suite, check, installer, or retained script
CI/release matrix -> scripts/tasks/check/platform/${platform} "${target}"
```

The release workflow's package step creates each matrix artifact; it is
distinct from the local `mise run package:release` suite, which forwards its
arguments to the release build atomic and then invokes the package-artifact
atomic without arguments. Atomics forward remaining arguments. The
`scripts/tasks/atomic/run-bounded` atomic requires GNU `timeout` on Linux and
enforces per-operation bounds with a kill-after margin. On macOS and Windows
it uses `gtimeout` when available; otherwise it explicitly relies on the
platform job's 60-minute bound rather than claiming silent per-operation
enforcement.

The native platform runners have explicit prerequisites. Linux musl runners
need `musl-tools` and `musl-gcc`; macOS needs an owned physical `RUNNER_TEMP`;
Windows needs the supported MSVC target and static CRT setup. macOS and
Windows do not claim Linux Docker certification.

Destructive Fedora VM/KVM certification is intentionally excluded from
automatic pre-push. The static policy inspection remains in `check:full`; run
the destructive entry point only on a prepared host:

```bash
mise run cert:vm
```

## Mise tasks

Mise routes each task to one existing executable script; aggregation remains in
the shell suites rather than inline task commands or dependencies.

| Task | Purpose |
|---|---|
| `mise run build` | debug build atomic |
| `mise run build:release` | release build atomic |
| `mise run test` | unit, integration, and e2e suite aggregate |
| `mise run test:unit` | library and unit-test atomic |
| `mise run test:integration` | every native `tests/*.rs` target once |
| `mise run test:e2e` | selected end-to-end suite |
| `mise run lint` | formatting and Clippy suite |
| `mise run dev` | bounded node-service smoke atomic |
| `mise run node` | authenticated node service atomic |
| `mise run cert` | transport, Health, and VM certification suite |
| `mise run cert:vm` | destructive Fedora VM certification |
| `mise run coverage` | deterministic coverage atomic |
| `mise run coverage:test` | offline coverage contract fixtures |
| `mise run usage:kdl` / `mise run usage:docs` | Usage artifact atomics |
| `mise run operation:catalog` | operation catalog atomic |
| `mise run package:release` | release packaging atomic |
| `mise run check:fast` / `mise run check:full` | canonical local gates |
| `mise run hooks:install` | configure local tracked Git hooks |

## Usage compatibility artifacts

Clap remains the sole source of truth for parsing, help, and shell
completions. The feature-gated `usage-kdl` binary uses the exact `clap_usage`
version, git URL, requested revision, and lock-resolved commit from
`Cargo.toml` and `Cargo.lock` to generate the presentation-only Usage artifact
under `docs/usage/`. It is not linked into the default `omakure` binary or its
completion generators.

The feature-gated `usage-docs` binary parses the checked
`docs/usage/omakure.kdl` and delegates Markdown and roff rendering to the
pinned official Usage renderers. It writes `docs/usage/omakure.md` and
`docs/usage/omakure.1`; these are deterministic checked-in artifacts covering
all 65 canonical CLI leaves. The renderer is never used at runtime and does
not make the shipped binary depend on a host path, timestamp, or external
runtime.

Run `mise run usage:kdl -- --review` when a Clap change may alter fidelity. It
prints added and removed losses plus a complete candidate allowlist; inspect
that report, update `docs/usage/fidelity-allowlist.json` manually only when
the change is reviewed, then run `mise run usage:kdl -- --write` followed by
`mise run usage:kdl -- --check`. Write and check are fail-closed: neither
auto-approves a changed loss nor overwrites a stale allowlist, residual
semantics record, or generated artifact. The checked residual for
`init script` is also exercised through actual Clap parser outcomes.

After KDL changes, run `mise run usage:docs -- --write` and then
`mise run usage:docs -- --check`. The check command fails if either generated
document is stale or missing. Both Usage checks and the operation catalog check
are required in CI.

The overlay is keyed by parity `entry_id` and `operation_family`, never by
Usage's rename-sensitive `full_cmd`.

Repository automation is under `scripts/tasks/atomic/`, `scripts/tasks/suite/`,
and `scripts/tasks/check/`. The latter exposes the four platform suites;
retained certification and developer implementations stay under
`scripts/tasks/cert/` and `scripts/tasks/dev/`. Installers are under
`scripts/install/`, release tooling under `scripts/release/`, and fixtures
under `scripts/fixtures/`. Installers never copy repository automation into a
workspace. Every resource-owning task is bounded and trap-cleaned, while
stateful install, node, release, and live certification tasks are repeat-safe
only under their documented preconditions.

## Architecture guide

- `src/domain/`: pure schema parsing, validation, and cron logic.
- `src/operations/`: shared behavior called by CLI and HTTP adapters.
- `src/cli/`: clap commands, JSON envelopes, API/node-service lifecycle, workers,
  scheduler, history, and local lifecycle commands.
- `src/runs.rs`: SQLite run state machine and trace storage.
- `src/run_executor.rs`: one execution path for direct, queued, and scheduled runs.
- `src/adapters/`: filesystem repository, process runner, environments, and
  runtime checks.
- `src/auth.rs`, `src/policy.rs`, `src/secrets.rs`, `src/redaction.rs`: deploy
  trust boundaries and secret handling.

The HTTP layer must remain an adapter. Add shared validation or behavior to an
operation, then cover it at the CLI/HTTP boundary as appropriate. Do not open
SQLite from route handlers or duplicate CLI logic in HTTP handlers.

## Focused tests

Use the canonical suites so local runs match hook and CI routing:

```bash
mise run test:unit
mise run test:integration
mise run test:e2e
mise run test:node-service
scripts/tasks/cert/transport
scripts/tasks/cert/health
mise run check:fast
```

`test:integration` is manifest-driven and runs each current `tests/*.rs`
basename exactly once. Platform matrix jobs use
`scripts/tasks/check/platform/{linux-gnu,linux-musl,macos,windows}` instead of
embedding test or build commands in workflow YAML.

## Certification toolchain

The certification gate requires Linux, Docker Engine 27 or newer, Docker
Compose v2.30 or newer, `jq` 1.8 or newer, SQLite 3.40 or newer, Cargo/Rust
matching the repository toolchain, and GNU `timeout`. Check the exact local
versions with:

The repository pins Rust `1.97.1` in `mise.toml` and CI. The production image
pins its Dockerfile frontend to
`docker/dockerfile:1@sha256:ecfaec9ed6d810b56388c508f4121597bfbba70d41a6dfeee4d8cad5f295fc32`,
uses the Debian Bookworm image digest in `Dockerfile`, and installs these exact
package versions: `bash=5.2.15-2+b13`, `ca-certificates=20250419~deb12u1`,
`curl=7.88.1-10+deb12u15`, `git=1:2.39.5-0+deb12u3`,
`jq=1.6-2.1+deb12u2`, and `tini=0.19.0-1+b3`. If Debian removes one of these
versions from its mutable mirrors, the image build fails rather than silently
selecting a different package.

```bash
docker version --format '{{.Client.Version}} {{.Server.Version}}'
docker compose version --short
jq --version
sqlite3 --version
cargo --version
rustc --version
timeout --version | head -n 1
```

The runtime image pins the Rust and Debian Bookworm base manifests by digest and
fails the build if the recorded apt package versions are unavailable. Record
the resulting image package manifest with `docker run --rm
omakure-node:transport-certification dpkg-query -W` when producing a release
image.

The integration suites launch the compiled binary, use temporary workspaces,
exercise real SQLite state, and test HTTP readiness/authentication. Keep
secrets out of assertions and logs. Use inline unit tests for pure parsers,
operations, state transitions, redaction, and runtime resolution.

## Release checks

CI and release jobs invoke the matrix-selected platform suite, which owns
native tests, target builds, static-link verification, and binary smoke. The
workflow files retain packaging/archive assertions but do not duplicate those
commands. Release archives are produced once per target and reuse the same
platform routing as CI. Before changing a command contract, run `omakure
help-ai` from the built binary and update `docs/ai-interface.md`,
`docs/cli-http-parity.md`, and the relevant tests.
