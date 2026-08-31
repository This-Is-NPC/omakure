# Development

Omakure is built and exercised as a headless CLI/HTTP application. Do not use
bare `omakure` as an application launch command: no-argument invocation only
prints help and returns; development entry points must name a command.

## Fast path

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
mise run dev:smoke
```

`mise run dev:smoke` builds the debug binary, starts a node service on a disposable
port, verifies `/v1/health` and `/v1/ready`, then terminates it. It does not
leave a daemon running or require a terminal UI. Set `OMAKURE_DEV_WORKSPACE`
and `OMAKURE_DEV_PORT` to override its fixtures.

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


## Usage compatibility artifacts

Clap remains the sole source of truth for parsing, help, and shell
completions. The feature-gated `usage-kdl` binary uses the exact `clap_usage`
version, git URL, requested revision, and lock-resolved commit from
`Cargo.toml` and `Cargo.lock` to generate the presentation-only Usage artifact
under `docs/usage/`. It is not linked into the default `omakure` binary or its
completion generators.

Run `mise run usage:kdl -- --review` when a Clap change may alter fidelity. It
prints added and removed losses plus a complete candidate allowlist; inspect
that report, update `docs/usage/fidelity-allowlist.json` manually only when
the change is reviewed, then run `mise run usage:kdl -- --write` followed by
`mise run usage:kdl -- --check`. Write and check are fail-closed: neither
auto-approves a changed loss nor overwrites a stale allowlist, residual
semantics record, or generated artifact. The checked residual for
`init script` is also exercised through actual Clap parser outcomes.

The check command is required in CI. The overlay is keyed by parity
`entry_id` and `operation_family`, never by Usage's rename-sensitive
`full_cmd`.

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
