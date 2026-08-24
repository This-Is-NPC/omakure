# Architecture

Omakure is a headless Rust application with three adapters over the same
workspace operations: the local CLI, the HTTP management API, and the
machine-owned single-process `node serve` service.

## Stack

| Layer | Technology | Purpose |
|---|---|---|
| Language | Rust 2021 | Portable application and CLI |
| CLI | clap 4.5, clap_complete 4.5 | Commands, help, and completions |
| Serialization | serde, serde_json, toml | Schemas, envelopes, config, policy |
| HTTP | axum 0.7, tokio 1, tower 0.5 | Authenticated management API |
| Storage | rusqlite 0.31, bundled SQLite | Runs, queue state, traces, search index |
| Errors | thiserror 1.0 | Typed domain and application errors |
| Scheduling | cron 0.12, chrono 0.4 | Schedule parsing and next-fire calculation |
| Processes | signal-hook 0.3, daemonize 0.5 | Graceful workers and Unix daemon mode |
| Security | argon2, subtle, sha2, rand | Token hashing, comparison, and generation |
| Filesystem | dirs 5, fs2 | Platform paths and file coordination |
| Windows | winreg 0.52 | Documents path and install-path handling |

Direct dependencies are intentionally limited to the retained headless surface.
The package does not declare `ratatui`, `crossterm`, `rattles`, or `mlua`.

## Source structure

```text
src/
├── main.rs                  CLI parsing, workspace resolution, dispatch
├── cli/                     command adapters and JSON output
│   ├── args.rs              clap command tree and long-form help
│   ├── api.rs               authenticated Axum management server
│   ├── node_service.rs      HTTP + workers + scheduler lifecycle
│   ├── run.rs               synchronous execution entry point
│   ├── queue.rs             queue producers and worker
│   ├── history.rs           run and trace queries
│   ├── serve.rs             standalone cron scheduler
│   ├── env.rs               managed environment commands
│   ├── battery.rs           Battery repository commands
│   ├── help_ai.rs           clap-derived machine surface
│   └── json.rs              stable envelope and error codes
├── domain/                  pure schema, parsing, validation, scheduling
├── operations/              protocol-neutral CLI/HTTP behavior
│   ├── core.rs              scripts, runs, queue, and workspace operations
│   ├── config.rs            resolved config and environment diagnostics
│   ├── doctor.rs            runtime and schema diagnostics
│   ├── envs.rs              managed environment operations
│   ├── scripts.rs           safe tree/content operations
│   ├── search.rs            indexed script search
│   └── battery.rs           sync, inspect, install, and provenance
├── adapters/                filesystem, process, environment, and checks
├── ports/                   repository and environment interfaces
├── runs.rs                  SQLite state machine and structured traces
├── run_executor.rs          shared child lifecycle and redaction
├── search_index.rs          SQLite full-text index
├── runtime.rs               Bash/PowerShell/Python detection and commands
├── workspace.rs             one workspace root and metadata layout
├── auth.rs                  token-file and legacy token authentication
├── policy.rs                deploy-time route and runtime policy
├── secrets.rs               secret references and provider resolution
├── redaction.rs             output and trace redaction
└── installer.rs             standalone installer binary
```

## Boundaries and invariants

- `domain/` is I/O-free. `operations/` owns validation and stable errors;
  CLI and HTTP only parse/render requests and responses.
- `runs.rs` is the sole owner of `runs.sqlite`. The state machine allows
  `queued`, `running`, `completed`, `failed`, `cancelled`, `timed_out`, and
  `dead_letter` with a closed transition graph.
- Direct runs, queue workers, and scheduled runs all use
  `run_executor::execute_with_heartbeat`, including cancellation, timeout,
  reserved environment variables, and output redaction.
 - `node serve` validates and initializes machine state before binding HTTP,
   then starts optional workers and scheduler and shuts
  them down in reverse order. `/v1/health` and `/v1/ready` are unauthenticated;
  other routes require bearer auth and policy scopes.
- Schedules are declared in script schemas. `serve` scans every five seconds,
  prevents overlapping fires, and records scheduler provenance in SQLite.
- A workspace is one filesystem root. `--scripts-dir` selects it; positional
  paths are not a command mode. Metadata is created only below that root.
- Script discovery supports `.bash`, `.sh`, `.ps1`, and `.py`; `.omakureignore`
  rules and metadata-directory exclusions are shared by CLI and HTTP listings.
- HTTP handlers never open SQLite or call CLI handlers directly. They call the
  same operations used by the CLI and map operation errors to HTTP statuses.
- `node.sqlite` is owned only by the node service and trust operations; it is
  never the workspace-owned `.history/runs.sqlite` database.
- The portable node foundation is complete. Transport, discovery, Nostr,
  enrollment, Pulses, remote Cues, campaigns, MDM, and Lua remain future
  features.

## Release and tests

CI runs all targets, clippy with warnings denied, formatting, packaging checks,
and release-readiness validation. Release archives contain only the matching
`omakure` executable (or `omakure.exe` on Windows). See
`release-artifacts.md` and `headless-release.md`.

Use `cargo test` for unit and integration coverage, `cargo clippy --all-targets
-- -D warnings` for lint, and `cargo tree --edges normal` to audit the retained
dependency graph.
