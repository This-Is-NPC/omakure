# Development

Omakure is built and exercised as a headless CLI/HTTP application. Do not use
bare `omakure` as an application launch command: no-argument invocation only
prints help and returns; development entry points must name a command.

## Checks and hooks

Install the tracked repository hooks from the repository root:

```bash
mise run hooks:install
```

This sets local Git `core.hooksPath` to `.githooks` and leaves unrelated
global Git configuration untouched. The hooks are thin wrappers around the
canonical scripts:

| Hook | Canonical script | Scope |
|---|---|---|
| pre-commit | `scripts/tasks/check/fast` | Shared static/fixture checks, then bounded formatting, Clippy, and `cargo test --lib --locked` |
| pre-push | `scripts/tasks/check/full` | The same shared checks once, then bounded formatting/Clippy, bounded all-target tests, and the complete locally executable Linux gate |

Run either script directly, or use the equivalent Mise tasks:

```bash
scripts/tasks/check/fast
scripts/tasks/check/full
mise run check:fast
mise run check:full
```

`check:fast` and `check:full` each invoke the internal
`scripts/tasks/check/shared` script exactly once. Fast then runs only
formatting, Clippy, and locked library tests; it does not run Docker, coverage
instrumentation, release builds, a network service, integration tests, or
end-to-end tests. The shared checks include shell/YAML syntax, lightweight
complexity contract fixtures, and the coverage contract fixtures.

Full does not invoke fast (which would duplicate its unit test); it runs the
shared checks once, then the canonical bounded formatting/Clippy script,
bounded locked all-target tests, bounded development service smoke, bounded
Usage KDL/Docs and operation catalog checks,
deterministic local coverage, complexity setup/corpus calibration/two-report
repeatability/ratchet/audit, packaging and a locked release build, VM policy
static checks, `scripts/tasks/cert/docker-smoke`, transport and Health
certification, and Health cleanup verification. Transport certification owns
its retained-suite cleanup verification and intentionally runs its own
`direct_transport_e2e` invocation in that certification context; the resulting
second run is deliberate. Other end-to-end suites are not repeated after
all-target tests or certification.

Both checks require the pinned Rust toolchain, Python with PyYAML, and GNU
`timeout`; GNU `timeout` is therefore also a pre-commit prerequisite. The full
check additionally requires Linux, Docker Engine and Compose, `jq`, and SQLite.
It is expected to be a long-running pre-push gate because it includes
deterministic instrumented coverage and bounded multi-container certification;
no duration estimate is promised. Commands run in order and stop at the first
failure. Certification scripts retain their own trap-backed cleanup when a gate
fails or is interrupted. Do not advertise bypassing hooks: resolve the missing
prerequisite or reported failure.

Destructive Fedora VM/KVM certification is intentionally not an automatic
pre-push gate. The static policy inspection remains in `check:full`; run the
explicit destructive task only on a prepared host with libvirt and its VM
prerequisites:

```bash
mise run cert:vm
```

## Mise tasks

| Task | Purpose |
|---|---|
| `mise run build` | `cargo build` |
| `mise run test` | unit, integration, and e2e test groups |
| `mise run lint` | `lint:fmt` and `lint:clippy` |
| `mise run dev:smoke` | bounded node-service health/readiness smoke check |
| `mise run node` | run the node service in the foreground |
| `mise run test:node-service` | focused CLI/HTTP/node-service integration tests |
| `mise run cert:transport` | bounded Linux transport certification |
| `mise run cert:health` | bounded Linux Health Plane certification |
| `mise run cert:vm-static` | static Fedora VM fixture checks |
| `mise run cert:vm` | bounded Fedora VM certification |
| `mise run coverage` | deterministic pinned LLVM HTML/LCOV/Cobertura reports plus the local baseline gate |
| `mise run coverage:test` | offline threshold, inventory, and normalization fixtures |
| `mise run install` | install the binary without copying repository scripts |
| `mise run usage:kdl` | generate or check pinned Clap-to-Usage compatibility artifacts |
| `mise run usage:docs` | generate or check Markdown and roff documentation from checked Usage KDL |
| `mise run operation:catalog` | generate or check the operation catalog artifacts |
| `mise run check:fast` | canonical cheap pre-commit checks |
| `mise run check:full` | canonical complete Linux pre-push suite |
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

Repository automation is under `scripts/tasks/`, `scripts/install/`,
`scripts/release/`, and `scripts/fixtures/`. `scripts/workspace/` is the
dedicated debug fixture selected by Cargo builds. External Battery repositories
own subject scripts; installers never copy repository automation into a
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

```bash
cargo test --test cli_surface_e2e
cargo test --test node_service_e2e
cargo test --test http_api_e2e
cargo test --test policy_e2e
cargo test --test packaging_smoke
cargo test --test direct_transport_contract
cargo test --test direct_transport_e2e
mise run cert:transport
mise run cert:health
```

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

CI runs `cargo test --all-targets --locked`, release builds for Linux/macOS/
Windows, clippy, formatting, packaging assertions, and release-note/version
validation. Before changing a command contract, run `omakure help-ai` from the
built binary and update `docs/ai-interface.md`, `docs/cli-http-parity.md`,
and the relevant tests.
