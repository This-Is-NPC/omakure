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
| Security | argon2, subtle, sha2, rand, k256, snow | Token hashing, BIP-340 identity, Noise transport, comparison, and generation |
| Resolution | hickory-resolver | Bounded async static-peer DNS resolution |
| Filesystem | dirs 5, fs2 | Platform paths and file coordination |
| Windows | winreg 0.52 | Documents path and install-path handling |

Direct dependencies are intentionally limited to the retained headless surface.
The package does not declare `ratatui`, `crossterm`, or `rattles`. It does
declare `mlua` (`lua54`, `vendored`), the embedded runtime for the `.lua`
script kind; the removed TUI widget runtime is unrelated and stays removed.

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
├── runtime.rs               Script-kind detection and command construction
├── workspace.rs             one workspace root and metadata layout
├── auth.rs                  token-file and legacy token authentication
├── policy.rs                deploy-time route and runtime policy
├── secrets.rs               secret references and provider resolution
├── redaction.rs             output and trace redaction
├── direct_transport.rs      Noise framing, certificates, envelopes, and replay limits
├── direct_service.rs        production direct listener and peer admission
├── discovery.rs              bounded trust-neutral LAN discovery
├── enrollment.rs             manual and signed-bundle enrollment records
├── node_transport.rs         node-owned transport state and static peers
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
- `.lua` is executed by a Lua runtime embedded in the binary, so a node runs
  Lua automation with nothing installed. It works by re-executing `omakure`
  itself as a Lua host, entered through an argv marker intercepted in `main`
  before the CLI parser. That marker is a maintainer-facing implementation
  detail: it is deliberately not a subcommand, because the global `--json` and
  `--scripts-dir` flags would otherwise consume a script's own arguments, and
  because a discoverable verb that executes an arbitrary file path is the wrong
  shape to expose ahead of authorized remote execution. Because the host is an
  ordinary child process, `run_executor` needed no change at all: timeout,
  cancel, heartbeat, capture, env injection and redaction apply unchanged.
- Script discovery supports `.bash`, `.sh`, `.ps1`, `.py`, and `.lua`; `.omakureignore`
  rules and metadata-directory exclusions are shared by CLI and HTTP listings.
- HTTP handlers never open SQLite or call CLI handlers directly. They call the
  same operations used by the CLI and map operation errors to HTTP statuses.
- `node.sqlite` is owned only by the node service and trust operations; it is
  never the workspace-owned `.history/runs.sqlite` database.
- The portable node foundation includes direct Noise transport, trust-neutral LAN
  discovery, manual enrollment, signed-bundle enrollment, static-peer lifecycle,
  revocation, replay protection, and bounded transport audit events. Nostr,
  campaigns and MDM remain future features.
- Remote Cues ship behind five fail-closed gates, every input read from the
  receiving node's own registry and config. A Cue names a script the Performer
  already declared in `trust.remote_cue_scripts` or `trust.remote_cue_batteries`
  and never carries one, so remote management can select among code a node
  already has and can never introduce more. Cue-origin runs execute with an
  explicit deny-all secret policy, are excluded from the worker lease steal so
  they run at most once, and report their provenance as `cue` rather than
  `manual`. See `.docs/remote-cue-contract.md`.
- The minimal Health Plane adds five application message kinds inside the frozen
  direct envelope — `health_profile`, `health_pulse`, `health_signal`,
  `health_ack`, `health_error` — and no new transport, signature construction,
  key material, or capability. `src/direct_health.rs` is the only seam between
  the shipped session and `src/health_plane/`, which owns authorization,
  ordering, idempotency, capacity, retention, and every bound.
  `src/operations/health.rs` projects the Conductor-local fleet-status and
  Signal-feed reports that `omakure node health` / `node signals` and
  `GET /v1/node/health` / `GET /v1/node/signals` both render. Health state is
  written only by the authenticated node-to-node exchange; CLI and HTTP are read
  surfaces and have no write path. Every quantitative bound is frozen in
  `.docs/health-plane-contract.md` and asserted by
  `tests/health_plane_contract.rs`.
- The baseline plane is the only one that carries code, so it is the only one
  authorized by two independent authorities: `src/baseline.rs` signs a versioned
  set of scripts under a publisher key held in `src/baseline_publisher.rs`, and
  `src/baseline_push.rs` will install one only for an active Conductor holding
  `baseline-push` *and* a publisher the receiver's own config names.
  `src/operations/baseline.rs` makes the install all-or-nothing on the
  filesystem, retains exactly one previous version, and re-runs the same
  verification when a node is rolled back onto it. `node_registry` refuses to
  let one node hold a publisher key and record a Performer, so authoring code
  and ordering it run stay two powers. Drift is a comparison the Health Plane
  projection makes from two facts the Performer reports; it is never a verdict
  the Performer sends. See `.docs/baseline-delivery.md`.

## Release and tests

CI runs all targets, the native protocol/build/lifecycle matrix, the native
Health Plane protocol/migration/lifecycle matrix, the bounded Linux multi-node
transport certification, the bounded Linux four-node Health Plane certification,
clippy with warnings denied, formatting, packaging checks, and release-readiness
validation. The two multi-container gates run on hosted Linux only; macOS and
Windows keep native coverage and never claim a container result. Release archives contain
only the matching `omakure` executable (or `omakure.exe` on Windows). See
`release-artifacts.md` and `headless-release.md`.

Use `cargo test` for unit and integration coverage, `cargo clippy --all-targets
-- -D warnings` for lint, and `cargo tree --edges normal` to audit the retained
dependency graph.
