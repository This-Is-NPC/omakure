# Architecture

## Tech Stack

| Layer | Technology | Version |
|-------|-----------|---------|
| Language | Rust | Edition 2021 |
| TUI Framework | ratatui | 0.26 |
| Terminal Backend | crossterm | 0.27 |
| CLI Parser | clap (derive) | 4.5 |
| Shell Completions | clap_complete | 4.5 |
| Serialization | serde + serde_json | 1.0 |
| Configuration | toml | 0.8 |
| Database | rusqlite (SQLite, bundled) | 0.31 |
| Scripting Engine | mlua (Lua 5.4, vendored) | 0.9 |
| Error Handling | thiserror | 1.0 |
| Cron Parsing | cron | 0.12 |
| Date/Time | chrono (`clock`+`std`) | 0.4 |
| Signal Handling | signal-hook | 0.3 |
| Daemonization (Unix) | daemonize | 0.5 |
| Duration Parsing | humantime | 2.1 |
| Platform Dirs | dirs | 5.0 |
| Loading Spinners | rattles | 0.2 |
| Windows Registry | winreg | 0.52 (Windows-only) |

## Dependencies

| Dependency | Version | Purpose |
|-----------|---------|---------|
| crossterm | 0.27 | Terminal input/output and raw-mode management |
| ratatui | 0.26 | TUI widget rendering |
| serde (+ derive) | 1.0 | Struct serialization/deserialization |
| serde_json | 1.0 | JSON parsing for schemas, history, and AI envelope |
| mlua (lua54, vendored) | 0.9 | Lua scripting for directory widgets |
| rusqlite (bundled) | 0.31 | Embedded SQLite for runs + search index |
| thiserror | 1.0 | Derive macro for error types |
| clap (derive) | 4.5 | CLI argument parsing; also feeds `help-ai` metadata |
| clap_complete | 4.5 | Generate shell completion scripts |
| toml | 0.8 | Theme and workspace configuration parsing |
| cron | 0.12 | Cron expression parser for the `omakure serve` scheduler |
| chrono | 0.4 | `DateTime<Utc>` for next-fire computations |
| signal-hook | 0.3 | Portable SIGINT/SIGTERM handling for workers + scheduler |
| daemonize (Unix) | 0.5 | `omakure serve --detach` double-fork on Unix |
| humantime | 2.1 | Parse `--timeout 30m`, `--since 1h` style durations |
| dirs | 5.0 | Platform-specific config/data directories |
| rattles | 0.2 | Themed loading spinners (braille frame source) |
| winreg (Windows) | 0.52 | Windows registry access for Documents path + PATH cleanup |

Dev-only: `rstest 0.23`, `insta 1.39`, `pretty_assertions 1.4`, `tempfile 3.10`.

## Project Structure

```
src/
├── main.rs                     Entry point, CLI dispatch, scripts-dir resolution
├── installer.rs                Standalone binary (omakure-installer)
├── app_meta.rs                 App version + repo URL constants
├── error.rs                    Centralized error hierarchy (AppError, SchemaError, …)
├── runs.rs                     SQLite run state machine + structured trace storage
├── run_executor.rs             Shared child-process lifecycle (run + worker + scheduler)
├── runtime.rs                  Script runtime detection (bash, ps1, py) + cmd builder
├── search_index.rs             SQLite-backed full-text search index
├── lua_widget.rs               Lua widget loader for directory-level widgets
├── theme_config.rs             Global theme config (~/.config/omakure/config.toml)
├── workspace.rs                Workspace layout: global root, .omaken, .history, envs
├── util.rs                     Shared filesystem helpers
├── domain/                     Pure domain (no I/O)
│   ├── schema.rs               Schema, Field, Schedule, QueueSpec structs
│   ├── parsing.rs              Embedded schema block extraction + JSON parsing
│   ├── validation.rs           Field input normalization and validation
│   └── schedule.rs             Cron expression normalization + next-fire computation
├── ports/                      Trait definitions (interfaces)
│   ├── mod.rs                  ScriptRepository, ScriptRunner
│   └── environment.rs          EnvironmentRepository, EnvironmentConfig
├── adapters/                   Concrete implementations
│   ├── workspace_repository.rs Filesystem-based ScriptRepository
│   ├── script_runner.rs        MultiScriptRunner (bash/ps1/py command construction)
│   ├── environments.rs         Filesystem EnvironmentRepository (+ sensitive masking)
│   ├── system_checks.rs        Runtime dep checks (git, bash, jq, python, pwsh)
│   ├── omarchy.rs              Omarchy terminal theme detection + import
│   └── tui/                    Terminal UI
│       ├── app.rs              App state machine, screens, in-place schedule toggle
│       ├── events.rs           Keyboard dispatch
│       ├── ui.rs               Layout + rendering dispatch
│       ├── theme.rs            Theme system (TOML + built-ins)
│       ├── state/              Per-screen state (navigation, search, history, …)
│       └── widgets/            Stateless renderers (scripts, history, dashboards,
│                                schedules, activity_grid, schema, search, …)
├── use_cases/                  Application services (ScriptService, EnvironmentService)
└── cli/                        CLI subcommand handlers
    ├── args.rs                 Clap definitions incl. global --json and long_about blocks
    ├── json.rs                 Single envelope writer + stable error codes
    ├── run.rs                  omakure run
    ├── queue.rs                omakure queue add|cancel|dead-letter|worker|stats
    ├── serve.rs                omakure serve (cron scheduler daemon + lock + tick loop)
    ├── serve_autostart.rs      systemd user unit install/uninstall/status (Linux-only)
    ├── history.rs              omakure history list|show|tail|stats|traces
    ├── trace.rs                omakure trace (in-script structured event writer)
    ├── describe.rs             omakure describe (AI verb)
    ├── search.rs               omakure search (AI verb)
    ├── list.rs                 omakure scripts (AI verb, --tag AND filter)
    ├── init.rs                 omakure init (--schema-json / --body-stdin / --force)
    ├── help_ai.rs              omakure help-ai (capability surface, 100% clap-derived)
    ├── config.rs               omakure config (resolved paths + --json)
    ├── doctor.rs               omakure doctor (deps + workspace + schema parse check)
    ├── theme.rs                omakure theme list|set|preview|path
    ├── update.rs               omakure update (GitHub releases self-update)
    ├── uninstall.rs            omakure uninstall [--scripts]
    └── omaken.rs               omakure list / omakure install (flavor management)
themes/                         Built-in TOML themes (default, dracula,
                                catppuccin-mocha, nord, solarized-dark)
scripts/                        Omakure workspace used as root in debug builds
.scripts/                       Repo dev/build helpers (dev-daemon.sh, consumed by mise)
.github/workflows/              CI/CD (ci.yml, release.yml, auto-release.yml)
tests/                          Integration tests (cli_positional_path.rs)
```

## Architectural Patterns

- **Hexagonal (Ports & Adapters):** `domain/` has no I/O; `ports/` defines traits (`ScriptRepository`, `ScriptRunner`, `EnvironmentRepository`); `adapters/` holds concrete impls. Composition happens in `use_cases/` and `cli/`.
- **SQLite as Runtime Source of Truth:** Every runtime fact (queue, history, traces, schedules) lives in `<workspace>/.history/runs.sqlite`. Config stays on disk; state stays in SQLite.
- **Run State Machine:** `runs.rs` gates transitions between `queued`, `running`, `completed`, `failed`, `cancelled`, `timed_out`, `dead_letter` via typed helpers (`enqueue`, `start_inline`, `claim_next`, `complete`, `fail`, `cancel`, `time_out`, `dead_letter`, `heartbeat`). Illegal moves return `invalid_argument`. `claim_next` uses a single atomic `UPDATE … RETURNING`.
- **Single Execution Code Path:** `omakure run`, `omakure queue worker`, and the scheduler-enqueued rows all flow through `run_executor::execute_with_heartbeat`. The helper spawns the child with `OMAKURE_RUN_ID`, refreshes the 60 s lease every 250 ms, reacts to mid-run cancel/timeout, and drains stdout/stderr via bounded mpsc channels so orphan grandchildren never deadlock the executor.
- **Native Cron Scheduler:** `cli/serve.rs` hosts a per-workspace daemon that rescans the workspace every 5 s, parses `Schedule` blocks, computes the next fire time with `domain/schedule.rs`, and enqueues runs through the same state machine (`trigger = Scheduled`, `cron_schedule_id = <canonical_path>@<cron_expr>`). Overlap protection: fires are skipped when a prior run with the same `cron_schedule_id` is still `queued`/`running`. Lock file at `.omaken/daemon.pid`; structured log at `.omaken/daemon.log`.
- **Per-Workspace Autostart:** `cli/serve_autostart.rs` renders a systemd user unit named `omakure-<fnv1a-hash-of-canonical-path>.service` so multiple workspaces coexist without collision (Linux-only).
- **Structured Trace Stream:** `cli/trace.rs` reads `OMAKURE_RUN_ID` from env and inserts into `run_traces` with monotonic per-run `sequence` computed inside a SQLite transaction. Reader is `history traces <run_id>` with `--level` + `--since-sequence` for incremental fetches. `PRAGMA foreign_keys = ON` cascades trace deletes when the parent run row is removed.
- **AI JSON Envelope:** All AI-facing verbs route through `cli/json.rs` emitting `{ ok, data, error, schema_version }`. Stable codes (`not_found`, `schema_invalid`, `script_exists`, `missing_required_field`, `invalid_argument`, `not_implemented`, `internal`) live in one place.
- **Help-AI Surface Auto-Derived:** `cli/help_ai.rs` walks `Cli::command()` to build its JSON payload, so `--help` text and the AI capability surface never drift.
- **Embedded Schema Convention:** Scripts embed their JSON schema between `OMAKURE_SCHEMA_START` / `OMAKURE_SCHEMA_END` comment markers. Parsing is byte-scoped to preserve the user's formatting; TUI toggle of `Schedule.Enabled` only rewrites the value, never the surrounding block.
- **State Machine TUI:** `adapters/tui/app.rs` centralizes state with a `Screen` enum (ScriptSelect, Search, Environments, FieldInput, History, Running, RunResult, Schedules, Error).
- **History Dashboards + Activity Grid:** History screen switches between `List`, `Dashboards` (BarChart of states, Sparkline over 14 days, per-script pie + duration sparkline), and an `ActivityGrid` heatmap aggregating run density over time. All views are pure functions of `app.history.entries` — no extra SQL.
- **Background Indexing:** `SearchIndex` rebuilds on a background thread using `Arc<Mutex<SearchStatus>>` polled by the TUI.
- **Theme System:** TOML themes loaded from `themes/` (bundled with `include_str!`) or `~/.config/omakure/themes/`. Omarchy system theme detected and imported via `adapters/omarchy.rs`.
- **Global Workspace vs. Session Scripts Root:** `Workspace` tracks a global root (owns `.history/`, `.omaken/`, `.omaken/envs/`, search-index, `omakure.toml`) and a separate scripts root. `omakure <PATH>` overrides only the scripts root; global state stays anchored to the platform default. History rows are keyed by absolute canonical script path so runs are addressable across both modes.
- **Themed Loading Spinners:** Single `App.tick` counter in `adapters/tui/mod.rs` drives braille frames from `rattles`. `Scan` theme for indexing; `Sand` theme for bootstrap/Lua/Running screens.

## Infrastructure

- **CI/CD:** GitHub Actions
  - `ci.yml` (PRs targeting `master`): `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, and release-readiness gate.
  - `release.yml`: matrix build (Linux x86_64, macOS x86_64, Windows x86_64) → tar.gz/zip → upload to GitHub Releases.
  - `auto-release.yml`: triggered on merge to `main` to auto-bump patch + tag + fire `release.yml`.
- **Task Runner (`mise.toml`):** `tui`, `dev` (build + daemon + TUI), `daemon-start`, `daemon-stop`, `daemon-log`, `build`, `test`, `lint`, `install`, `coverage` (tarpaulin).
- **Two Binaries:** `omakure` (main TUI + CLI) and `omakure-installer` (standalone installer).
- **Install Scripts:** `install.sh`, `install.ps1`, `install-from-source.sh` at the repo root (public `curl | bash` entrypoints).
- **License:** AGPL-3.0-only.

## Code Metrics

| Metric | Status | Value / Finding | Source (tool + command) or Recommendation |
|--------|--------|-----------------|-------------------------------------------|
| Test structure | measured | `#[cfg(test)] mod tests` inline per source file; 638 test fns across 67 files; 1 integration file in `tests/cli_positional_path.rs`; frameworks: stdlib `#[test]`, `rstest 0.23`, `insta 1.39`, `pretty_assertions 1.4`, `tempfile 3.10`. | `cargo test` / `mise run test` |
| Test coverage | measured | 81.36% (4693/5768 lines) | `mise run coverage` → `cargo tarpaulin --out Html --exclude-files 'src/installer.rs' 'src/app_meta.rs' 'src/adapters/tui/mod.rs' 'src/main.rs' 'src/cli/trace.rs' 'src/cli/doctor.rs' 'src/cli/uninstall.rs' 'src/cli/omaken.rs' 'src/cli/update.rs' --skip-clean` |
| Module sizes (LOC) | measured | 25,322 total across `src/`. Top-5 files: `adapters/tui/app.rs` 2,464 · `runs.rs` 2,377 · `adapters/tui/widgets/dashboards.rs` 968 · `cli/history.rs` 957 · `cli/queue.rs` 956. | `find src -name '*.rs' \| xargs wc -l` |
| Cyclomatic complexity | recommended | — | No tool configured. For Rust, install `lizard` (polyglot, fast, supports `.rs`) — `pipx install lizard && lizard src/ -l rust -C 15` — or enable Clippy's complexity lints: `cargo clippy -- -W clippy::cognitive_complexity -W clippy::cyclomatic_complexity`. Lizard gives numeric hotspots; Clippy is zero-install but per-function only. |
| Internal dependency structure | recommended | — | No tool configured. For Rust, install `cargo-modules` (`cargo install cargo-modules`) and run `cargo modules structure --bin omakure` for a tree view, or `cargo modules dependencies --bin omakure --layout dot \| dot -Tsvg -o deps.svg` for a graph. Alternative: `cargo-depgraph` for crate-level only. |
| Mutation score | recommended | — | No tool configured. For Rust install `cargo-mutants` (`cargo install cargo-mutants`) and run `cargo mutants --timeout 120 --in-diff origin/test..HEAD` for PR mutation testing; wider audit with `cargo mutants -- --lib` (slow — expect hours on this codebase). Fits the stack natively; no Rust alternative is as mature. |
