# AGENTS.md

Guidelines for working in the Omakure codebase.

## Product overview

Omakure is a headless Rust automation runner. Its supported surfaces are the
CLI, the authenticated HTTP management API, and the machine-owned `node serve`
process.
The CLI and HTTP adapters call shared protocol-neutral operations. There is no
interactive terminal application, theme subsystem, or directory widget
runtime.

**Key concepts:**

- **Workspace**: one selected root containing Battery-installed subject scripts
  and Omakure metadata. Default: `~/Documents/omakure-scripts`.
- **Repository automation**: executable tasks, installers, release tooling, and
  fixtures live below `scripts/`; they are not product subjects.
- **Battery ownership**: subject-script collections come only from external
  Battery repositories and are installed explicitly into a workspace.
- **Schema**: PascalCase JSON embedded between
  `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`.
- **Runtime state**: `.history/runs.sqlite` stores runs, queue state, and traces.
- **Metadata**: `.omakure/` stores environments, Battery registry/cache,
  scheduler artifacts, and workspace-owned runtime files.
- **Node service**: HTTP plus optional queue workers and scheduler in one
  machine-owned process with isolated identity/trust state.

## Task and plan management

Use the Omakiten MCP for shaping, planning, task tracking, and workflow moves.
Tasks, plans, waves, and dependencies are project-scoped. Inspect with
`project.overview`, `tasks.list`, and `plans.show`; use `okt-task-continue` or
`okt-run` for approved work. Do not mix projects or bypass explicit workflow
transitions. GitHub tracking is separate; see `CONTRIBUTING.md`.

## Build and run

```bash
cargo build
cargo run -- --help

# Validation gates (required before declaring work done)
mise run check:fast         # pre-commit: fmt, clippy, library tests, static fixtures
mise run check:full         # pre-push / Linux GNU CI: complete locally executable suite

# Named headless development workflows
mise run dev:smoke          # bounded node-service health/readiness smoke check
mise run node               # foreground node service
mise run test:node-service  # focused CLI/HTTP/node-service integration tests
```

Never use bare `omakure` as an interactive app. No-argument invocation prints
help; operational commands must be explicit (`omakure scripts`, `omakure run`,
`omakure api`, or `omakure node serve`). The `scripts/tasks/atomic/dev-smoke` atomic
(`mise run dev:smoke`) starts the node service on a local port
(`OMAKURE_DEV_PORT`, default 17878), checks health/readiness, and cleans up.

## Workspace selection

`--scripts-dir` is the supported explicit override. Resolution then considers
`OMAKURE_SCRIPTS_DIR`, the debug `scripts/workspace` fixture, and the platform
default `~/Documents/omakure-scripts`. Positional script paths are not
accepted.

The debug build uses `scripts/workspace` when it exists. Omakure creates
`.omakure/`, `.history/`, and `omakure.toml` only below the selected workspace.
Repository automation under `scripts/tasks`, installers, release tooling, and
fixtures is never a Battery subject.

## Architecture

`docs/internal/architecture.md` is the canonical module map, stack, and
invariant list; update it with any structural change. In summary:

```text
src/
├── main.rs, lib.rs, bin/    # binary entry, crate surface, doc/catalog generators
├── cli/                     # clap adapters (args, api, node_service, node, run,
│                            #   queue, history, serve, env, battery, help_ai,
│                            #   inventory, json, …)
├── operations/              # protocol-neutral behavior shared by CLI and HTTP
├── domain/                  # pure schemas, parsing, validation, cron, node config
├── adapters/, ports/, use_cases/  # filesystem/process adapters and interfaces
├── runs/, run_executor.rs   # runs.sqlite state machine; shared child lifecycle
├── runtime.rs, search_index.rs, workspace.rs
├── auth.rs, policy.rs, secrets.rs, redaction.rs
├── cli_http_parity.rs, operation_catalog.rs  # versioned parity and operation catalogs
├── installer.rs             # standalone installer binary
└── fleet planes:
    ├── node.rs, node_identity.rs, node_transport.rs  # node state and identity
    ├── node_registry/ (+ health/)        # node.sqlite trust/health persistence
    ├── direct_transport.rs, direct_service/  # Noise transport and listener
    ├── discovery.rs                      # trust-neutral LAN discovery
    ├── enrollment.rs, enrollment_authority.rs  # manual/signed enrollment
    ├── health_plane/, direct_health.rs   # Health Plane domain and carriage
    ├── remote_cue.rs                     # Cue plane receive half
    └── baseline.rs, baseline_push.rs, baseline_publisher.rs  # Baseline plane
```

### Boundaries

- Keep I/O out of `domain/`.
- Put validation, path confinement, and stable operation errors in
  `operations/`, not in one adapter only.
- HTTP handlers must call operations; they must not call CLI modules or open
  SQLite directly.
- Direct runs, queue workers, and scheduled runs must use
  `run_executor::execute_with_heartbeat`.
- `runs/` is the sole owner of `.history/runs.sqlite` access.
- Keep Omakure-reserved variables and secret redaction rules centralized.

## Dependencies

Runtime dependencies are exactly those in `Cargo.toml`: `mlua`, `serde`,
`serde_json`, `rusqlite`, `thiserror`, `clap`, `clap_complete`, `toml`,
`humantime`, `signal-hook`, `cron`, `chrono`, `axum`, `tokio`, `tower`,
`serde_urlencoded`, `subtle`, `sha2`, `k256`, `argon2`, `rand`, `fs2`, `snow`,
`hickory-resolver`, `curve25519-dalek`, `serde_jcs`, and `tempfile`; Unix-only
`daemonize` and `libc`; Windows-only `winreg` and `windows-sys`. `clap_usage`
and `usage-lib` are optional and enabled only by the `usage-generator` feature.
The stack table in `docs/internal/architecture.md` records what each one is
for. The headless package must not reintroduce `ratatui`, `crossterm`, or
`rattles`. `mlua` is declared deliberately and must stay: it is the embedded
runtime for the `.lua` script kind, which is a different Lua from the removed
TUI widget runtime.

## Script schema

```text
# OMAKURE_SCHEMA_START
# {
#   "Name": "deploy",
#   "Description": "Deploy the service",
#   "Tags": ["ops"],
#   "Fields": [{"Name":"target","Type":"string","Required":true,"Arg":"--target"}]
# }
# OMAKURE_SCHEMA_END
```

Supported script extensions are `.bash`, `.sh`, `.ps1`, `.py`, and `.lua`.
Schema fields may be strings, numbers, booleans, or secrets. Optional
`Schedule` data is consumed by `serve` and `node serve`; see `docs/scheduling.md`.

## JSON and HTTP contracts

AI-facing CLI commands support `--json` and emit
`{ ok, data, error, schema_version }`. `help-ai` always emits JSON and is
generated from clap metadata. HTTP health/readiness are unauthenticated;
other routes require a scoped Argon2id bearer token from a `--tokens-file`.

## Testing

Declare work done only after `mise run check:fast` passes.

Before claiming Linux GNU CI will pass, run `mise run check:full`. At minimum,
run `./scripts/tasks/atomic/overlay-fs-lib` and
`cargo test --lib --locked test_dependency_checks` (overlay install and
PATH/shim invariants). `cargo test` alone does not match Linux GNU CI because
it skips overlay-backed install checks and other platform invariants.

Changes that touch PATH resolution, overlay installs, symlinks, or similar
environment invariants must land the matching overlay or dependency tests in
the same change.

Targeted slices (not validation gates):

```bash
cargo test --test cli_surface_e2e
cargo test --test node_service_e2e
cargo test --test http_api_e2e
cargo test --test packaging_smoke
cargo test --lib --locked test_dependency_checks
```

Unit tests are inline. Integration tests launch the compiled binary and use
temporary workspaces. Keep secrets out of test output. Packaging tests verify
that removed UI/theme/widget assets and dependencies are absent and that
release archives contain only the binary. See `docs/internal/development.md`
for hook routing and platform suite layout.

## Release

GitHub Actions builds Linux, macOS, and Windows headless binaries from version
tags. CI requires tests, clippy, formatting, and package checks. Release notes
are generated by GitHub from the commits since the previous tag, so a release
needs no hand-written file. Archives contain only `omakure` or `omakure.exe`.
See `docs/internal/release-artifacts.md`.
