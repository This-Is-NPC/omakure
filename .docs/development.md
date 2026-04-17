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
PID file at `<workspace>/.omaken/daemon.pid`, structured log at
`<workspace>/.omaken/daemon.log`. See `usage.md` and
`how-to-create-a-script.md#schedule-optional` for usage.

## Testing

- Unit tests are inline (`#[cfg(test)] mod tests`) in each source file.
- Integration tests live in `tests/` (currently `cli_positional_path.rs`).
- `cargo test` / `mise run test` runs everything; `mise run coverage` produces `tarpaulin-report.html`.
- No helper framework beyond `rstest 0.23`, `insta 1.39`, `pretty_assertions 1.4`, `tempfile 3.10`.

## CI

- `.github/workflows/ci.yml` runs on every PR to `master`: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, release-readiness gate.
- `.github/workflows/release.yml` builds the cross-platform matrix and attaches release archives.
- `.github/workflows/auto-release.yml` bumps patch + tags on merge to `main`, triggering the release workflow.
