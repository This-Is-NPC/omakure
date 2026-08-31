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
cargo test
cargo run -- --help

# Named headless development workflows
mise run dev:smoke          # bounded node-service health/readiness smoke check
mise run node               # foreground node service
mise run test:node-service  # focused CLI/HTTP/node-service integration tests
mise run lint               # clippy and fmt checks
```

Never use bare `omakure` as an interactive app. No-argument invocation prints
help; operational commands must be explicit (`omakure scripts`, `omakure run`,
`omakure api`, or `omakure node serve`). The `scripts/tasks/dev/smoke` helper
starts the node service on a temporary local port, checks health/readiness, and
cleans up.

## Workspace selection

`--scripts-dir` is the supported explicit override. Resolution then considers
`OMAKURE_SCRIPTS_DIR`, legacy `OVERTURE_SCRIPTS_DIR` and
`CLOUD_MGMT_SCRIPTS_DIR`, the debug `scripts/workspace` fixture, platform
defaults, and legacy default directory names. Positional script paths are not
accepted.

The debug build uses `scripts/workspace` when it exists. Omakure creates
`.omakure/`, `.history/`, and `omakure.toml` only below the selected workspace.
Repository automation under `scripts/tasks`, installers, release tooling, and
fixtures is never a Battery subject.

## Architecture

```text
src/
├── cli/                  # clap command adapters and JSON output
│   ├── args.rs           # command tree and long-form help
│   ├── api.rs            # authenticated HTTP adapter
│   ├── node_service.rs   # HTTP + workers + scheduler lifecycle
│   ├── run.rs            # direct execution
│   ├── queue.rs          # queue producers and workers
│   ├── history.rs        # run and trace reads
│   ├── serve.rs          # cron scheduler
│   ├── env.rs            # environment management
│   ├── battery.rs        # Battery management
│   ├── help_ai.rs        # clap-derived machine surface
│   └── json.rs           # stable envelope/errors
├── domain/               # pure schemas, parsing, validation, cron
├── operations/           # shared behavior used by CLI and HTTP
├── adapters/             # filesystem, process, env, runtime checks
├── ports/                # repository and environment interfaces
├── runs.rs               # SQLite state machine and trace storage
├── run_executor.rs       # shared child lifecycle and redaction
├── search_index.rs       # SQLite full-text search
├── runtime.rs            # Bash/PowerShell/Python command construction
├── workspace.rs          # one-root workspace layout
├── auth.rs/policy.rs     # tokens and deploy policy
├── secrets.rs/redaction.rs
└── installer.rs          # standalone installer binary
```

### Boundaries

- Keep I/O out of `domain/`.
- Put validation, path confinement, and stable operation errors in
  `operations/`, not in one adapter only.
- HTTP handlers must call operations; they must not call CLI modules or open
  SQLite directly.
- Direct runs, queue workers, and scheduled runs must use
  `run_executor::execute_with_heartbeat`.
- `runs.rs` is the sole owner of `.history/runs.sqlite` access.
- Keep Omakure-reserved variables and secret redaction rules centralized.

## Dependencies

Retained runtime dependencies include `clap`, `clap_complete`, `serde`,
`serde_json`, `toml`, `rusqlite`, `axum`, `tokio`, `tower`, `thiserror`, `cron`,
`chrono`, `signal-hook`, `daemonize`, `humantime`, `dirs`, `fs2`, `argon2`,
`subtle`, `sha2`, `rand`, and Windows-only `winreg`. The headless package must
not reintroduce `ratatui`, `crossterm`, or `rattles`. `mlua` is declared
deliberately and must stay: it is the embedded runtime for the `.lua` script
kind, which is a different Lua from the removed TUI widget runtime.

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
other routes require bearer auth. Prefer scoped Argon2id tokens from a
`--tokens-file`; legacy `OMAKURE_API_TOKEN` mode is for local compatibility.

## Testing

```bash
cargo test
cargo test --test cli_surface_e2e
cargo test --test node_service_e2e
cargo test --test http_api_e2e
cargo test --test packaging_smoke
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Unit tests are inline. Integration tests launch the compiled binary and use
temporary workspaces. Keep secrets out of test output. Packaging tests verify
that removed UI/theme/widget assets and dependencies are absent and that
release archives contain only the binary.

## Release

GitHub Actions builds Linux, macOS, and Windows headless binaries from version
tags. CI requires tests, clippy, formatting, and package checks. Release notes
are generated by GitHub from the commits since the previous tag, so a release
needs no hand-written file. Archives contain only `omakure` or `omakure.exe`.
See `docs/internal/release-artifacts.md`.
