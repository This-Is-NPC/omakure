# Development

## How to run in development

```bash
cargo run                   # Build + open the TUI
mise run dev                # Build, start scheduler daemon, tail log, open TUI
mise run tui                # Open the TUI only
```

Common TUI shortcuts (all cross-screen navigation uses `Ctrl+/` prefix):

- `Ctrl+/` then `s` — search scripts (background-indexed)
- `Ctrl+/` then `e` — environment selector
- `Ctrl+/` then `h` — run history (List / Dashboards / Activity grid)
- `Ctrl+/` then `c` — schedules screen; `Space` toggles `Schedule.Enabled` in place
- `Ctrl+/` then `q` — quit

In debug builds, the app uses the repo `scripts/` folder as the workspace
root if it exists. To override the scripts location, set
`OMAKURE_SCRIPTS_DIR=/path/to/scripts`. See `scripts-path.md` for the full
precedence chain.

## Task runner (`mise`)

| Task | Purpose |
| --- | --- |
| `mise run build` | `cargo build` |
| `mise run test` | `cargo test` |
| `mise run lint` | `cargo clippy --all-targets -- -D warnings` + `cargo fmt --check` |
| `mise run coverage` | `cargo tarpaulin --out Html` (excludes installer, trace, update, etc.) |
| `mise run dev` | Build + start daemon + tail log + launch TUI; stops daemon on exit |
| `mise run daemon-start` / `daemon-stop` / `daemon-log` | Manage `omakure serve -d` standalone |
| `mise run install` | `cargo install --path .` |

Repo-level shell helpers live under `.scripts/` (only `.scripts/dev-daemon.sh` for
now — invoked by `mise run dev`). The top-level `scripts/` folder is the
Omakure workspace used in debug builds and is **not** meant for repo
tooling.

## Architecture (Rust code)

Ports-and-adapters (hexagonal):

- `src/domain/` — pure types and parsing. No I/O.
  - `schema.rs` — `Schema`, `Field`, `Schedule`, `QueueSpec`, `OutputField`.
  - `parsing.rs` — extract the `OMAKURE_SCHEMA_START`/`END` block and parse JSON.
  - `validation.rs` — field input normalization.
  - `schedule.rs` — cron expression normalization + `next_fire_after`.
- `src/ports/` — trait interfaces (`ScriptRepository`, `ScriptRunner`, `EnvironmentRepository`).
- `src/adapters/` — concrete I/O impls: filesystem repository, process runner, Omarchy theme detection, TUI.
- `src/use_cases/` — application services (`ScriptService`, `EnvironmentService`).
- `src/cli/` — subcommand handlers; clap lives in `cli/args.rs`, JSON envelope in `cli/json.rs`.
- `src/runs.rs` — SQLite run state machine + `run_traces` structured event storage.
- `src/run_executor.rs` — shared child-process lifecycle (used by `omakure run`, `queue worker`, and scheduler fires).
- `src/search_index.rs` — SQLite-backed script search with background rebuild.
- `src/workspace.rs` — global-root vs. session-scripts-root invariants.

## Scheduler workflow

`omakure serve` scans the workspace every 5 s, parses each script's
`Schedule` block, and enqueues runs through the same state machine as
manual runs (`trigger = Scheduled`). Overlap protection skips fires when
a prior run with the same `cron_schedule_id` is still `queued`/`running`.
PID file at `<workspace>/.omakure/daemon.pid`, structured log at
`<workspace>/.omakure/daemon.log`. See `usage.md` and
`how-to-create-a-script.md#schedule-optional` for usage.

## Testing

- Unit tests are inline (`#[cfg(test)] mod tests`) in each source file.
- Integration tests live in `tests/` and include CLI/path coverage (`cli_positional_path.rs`, `cli_battery.rs`, `spike_command_path_resolution.rs`) plus the black-box surface suite (`secret_cli_e2e.rs`, `http_api_e2e.rs`, `support_harness.rs`).
- `cargo test` / `mise run test` runs everything; `mise run coverage` produces `tarpaulin-report.html`.
- No helper framework beyond `rstest 0.23`, `insta 1.39`, `pretty_assertions 1.4`, `tempfile 3.10`.

### Black-box surface suite

The black-box surface suite exercises shipped process and network boundaries instead of calling Rust internals directly:

- `tests/secret_cli_e2e.rs` runs the compiled `omakure` binary through `run`, `env`, `queue`, and `history` JSON flows, using temporary workspaces and bounded child-process timeouts.
- `tests/http_api_e2e.rs` starts the compiled `omakure api` server on loopback, drives the HTTP API with a small stdlib TCP client, runs the queue worker when needed, and tears the server down through a child-process guard.
- `tests/support_harness.rs` covers the shared integration harness: temporary workspaces, JSON envelope parsing, secret assertions, HTTP readiness, and process teardown.

Run the focused suite while iterating:

```bash
cargo test --test secret_cli_e2e
cargo test --test http_api_e2e
```

Use module tests for parser, validator, redaction, operation, state-machine, and pure rendering behavior where the public CLI/HTTP boundary is not required. Keep E2E cases for cross-boundary regressions that require the real binary, SQLite workspace state, child-process execution, or HTTP authorization/status mapping.

E2E secrets are canaries. They must never appear in stdout, stderr, API responses, persisted history rows, or assertion failure output. Prefer length/status-only assertion messages for commands that might carry canary data.

No new dev-dependency was added for this suite. It uses the existing test-only `tempfile` dependency for automatic temporary workspace cleanup and stdlib process/TCP helpers for bounded execution and HTTP checks.

## CI

- `.github/workflows/ci.yml` runs on every PR to `master`: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, release-readiness gate.
- `.github/workflows/release.yml` builds the cross-platform matrix and attaches release archives.
- `.github/workflows/auto-release.yml` runs when a PR is merged into `master`, reads the current `Cargo.toml` version, requires matching release notes, creates the tag, and triggers the release workflow.
