# Architecture

## Tech Stack

| Layer | Technology | Version |
|-------|-----------|---------|
| Language | Rust | Edition 2021 |
| TUI Framework | ratatui | 0.26 |
| Terminal Backend | crossterm | 0.27 |
| CLI Parser | clap (derive) | 4.5 |
| Serialization | serde + serde_json | 1.0 |
| Configuration | toml | 0.8 |
| Database | rusqlite (SQLite, bundled) | 0.31 |
| Scripting Engine | mlua (Lua 5.4, vendored) | 0.9 |
| Error Handling | thiserror | 1.0 |
| Shell Completions | clap_complete | 4.5 |
| Platform Dirs | dirs | 5.0 |
| Signal Handling | signal-hook | 0.3 |
| Duration Parsing | humantime | 2.1 |
| Loading Spinners | rattles | 0.2 |
| Windows Registry | winreg (Windows only) | 0.52 |

## Dependencies

| Dependency | Version | Purpose |
|-----------|---------|---------|
| crossterm | 0.27 | Terminal input/output and raw mode |
| ratatui | 0.26 | TUI widget rendering |
| serde | 1.0 (derive) | Struct serialization/deserialization |
| serde_json | 1.0 | JSON parsing for script schemas and history |
| mlua | 0.9 (lua54, vendored) | Lua scripting for custom TUI widgets |
| rusqlite | 0.31 (bundled) | SQLite-based search index |
| thiserror | 1.0 | Derive macro for error types |
| clap | 4.5 (derive) | CLI argument parsing |
| clap_complete | 4.5 | Shell completion generation |
| toml | 0.8 | Theme and workspace configuration parsing |
| dirs | 5.0 | Platform-specific config/data directories |
| signal-hook | 0.3 | Portable SIGINT/SIGTERM handling for `queue worker` |
| humantime | 2.1 | Parse `--timeout 30m`, `--since 1h` style durations |
| rattles | 0.2 | Themed loading spinners (braille frame source) |
| winreg | 0.52 | Windows registry access for Documents path |

## Project Structure

```
src/
├── main.rs                  # Entry point, CLI dispatch, TUI bootstrap
├── installer.rs             # Standalone binary for omakure-installer
├── app_meta.rs              # App version and repo URL constants
├── error.rs                 # Centralized error types (AppError, SchemaError, ScriptError, EnvironmentError)
├── runs.rs                  # SQLite-backed run state machine + structured trace storage
├── run_executor.rs          # Shared execution helper used by both `omakure run` and the worker
├── runtime.rs               # Script runtime detection (bash, ps1, py) and command builder
├── search_index.rs          # SQLite-backed full-text search index
├── lua_widget.rs            # Lua widget loader for custom directory widgets
├── theme_config.rs          # Global theme configuration (config.toml management)
├── workspace.rs             # Workspace layout: root, .omaken, .history, envs
├── util.rs                  # Shared filesystem helpers
├── domain/                  # Core domain logic (no I/O dependencies)
│   ├── schema.rs            # Schema, Field, OutputField, QueueSpec structs
│   ├── parsing.rs           # Schema block extraction and JSON parsing
│   └── validation.rs        # Field input normalization and validation
├── ports/                   # Trait definitions (interfaces)
│   ├── mod.rs               # ScriptRepository, ScriptRunner traits
│   └── environment.rs       # EnvironmentRepository trait, EnvironmentConfig
├── adapters/                # Concrete implementations
│   ├── workspace_repository.rs  # Filesystem-based ScriptRepository
│   ├── script_runner.rs     # MultiScriptRunner (bash, ps1, py execution)
│   ├── environments.rs      # Filesystem-based EnvironmentRepository
│   ├── system_checks.rs     # Runtime dependency checks (git, bash, jq, python, pwsh)
│   └── tui/                 # Terminal UI module
│       ├── app.rs           # App state machine, screen navigation, all app logic
│       ├── events.rs        # Keyboard event handling
│       ├── ui.rs            # Layout and rendering dispatch
│       ├── theme.rs         # Theme system: loading, parsing, built-in themes
│       ├── state/           # Per-screen state structs
│       │   ├── navigation.rs
│       │   ├── search.rs
│       │   ├── history.rs
│       │   ├── environment.rs
│       │   └── field_input.rs
│       └── widgets/         # Stateless rendering widgets
│           ├── scripts.rs
│           ├── schema.rs
│           ├── search.rs
│           ├── history.rs
│           ├── dashboards.rs
│           ├── field_input.rs
│           ├── environment.rs
│           ├── envs.rs
│           ├── running.rs
│           ├── run_result.rs
│           ├── error.rs
│           ├── loading.rs
│           ├── spinner.rs
│           └── common.rs
├── use_cases/               # Application services
│   ├── mod.rs               # ScriptService (list, load schema, run)
│   └── environment.rs       # EnvironmentService (list, load config, set active)
└── cli/                     # CLI subcommand handlers
    ├── args.rs              # Clap argument definitions (incl. global --json)
    ├── json.rs              # Single JSON envelope writer + stable error codes
    ├── run.rs               # `omakure run <script>` (+ --actor/--reason/--json/--no-prompt)
    ├── doctor.rs            # `omakure doctor` runtime checks
    ├── describe.rs          # `omakure describe <script>` AI verb
    ├── search.rs            # `omakure search <query>` AI verb
    ├── history.rs           # `omakure history list|show|tail|stats|traces` (state filter, traces reader)
    ├── help_ai.rs           # `omakure help-ai` capability discovery
    ├── list.rs              # `omakure scripts` list (+ --tag AND filter, + --json)
    ├── init.rs              # `omakure init` create script template (+ --schema-json/--body-stdin/--force)
    ├── config.rs            # `omakure config` show resolved paths (+ --json)
    ├── omaken.rs            # `omakure list/install` flavor management
    ├── queue.rs             # `omakure queue add|cancel|dead-letter|worker|stats`
    ├── trace.rs             # `omakure trace` script-side structured trace writer
    ├── theme.rs             # `omakure theme` list/set/preview themes
    ├── update.rs            # `omakure update` self-update from GitHub
    └── uninstall.rs         # `omakure uninstall` remove binary
themes/                      # Built-in theme TOML files (default, dracula, catppuccin-mocha, nord, solarized-dark)
scripts/                     # Development scripts directory (workspace root in debug)
.github/workflows/           # CI/CD pipelines
```

## Architectural Patterns

- **Hexagonal Architecture (Ports & Adapters):** Core domain logic in `domain/` has no I/O. Traits in `ports/` define boundaries (`ScriptRepository`, `ScriptRunner`, `EnvironmentRepository`). Concrete implementations in `adapters/` (filesystem, process execution, TUI).
- **State Machine TUI:** The `App` struct in `adapters/tui/app.rs` acts as a centralized state machine with a `Screen` enum driving navigation between ScriptSelect, Search, Environments, FieldInput, History, Running, RunResult, and Error screens.
- **Service Layer:** `use_cases/` contains `ScriptService` and `EnvironmentService` that compose port traits, decoupling CLI/TUI from concrete adapters.
- **Embedded Schema Convention:** Scripts embed their schema as JSON inside comment blocks (`OMAKURE_SCHEMA_START`/`OMAKURE_SCHEMA_END`), parsed at runtime.
- **Background Indexing:** `SearchIndex` rebuilds a SQLite index on a background thread, using `Arc<Mutex<SearchStatus>>` for status communication.
- **AI JSON Envelope:** All AI-facing CLI verbs route their output through a single helper in `src/cli/json.rs` that emits a uniform `{ ok, data, error, schema_version }` payload. Stable error codes (`not_found`, `schema_invalid`, `script_exists`, `missing_required_field`, `invalid_argument`, `not_implemented`, `internal`) live in one place so the contract cannot drift verb-by-verb.
- **SQLite Run Log + State Machine:** Run history is persisted in `<workspace>/.history/runs.sqlite` via `src/runs.rs`. The `runs` table doubles as "what is happening now" and "what already happened" via the `state` column (`queued`, `running`, `completed`, `failed`, `cancelled`, `timed_out`, `dead_letter`). State transitions are gated by typed helpers (`enqueue`, `start_inline`, `claim_next`, `complete`, `fail`, `cancel`, `time_out`, `dead_letter`, `heartbeat`) so illegal moves surface as `error.code = "invalid_argument"`. The atomic claim is one `UPDATE … RETURNING` SQLite statement, guaranteeing two workers (or two threads of the same worker) never claim the same row. On first open, two destructive cleanups run: legacy `.history/*.json` files are deleted, and a v0.1-shaped `runs` table (no `state` column) is dropped and recreated with the new schema.
- **Single Execution Code Path:** Both `omakure run` (synchronous fast path) and `omakure queue worker` (daemon draining the queue) drive their child processes through `src/run_executor.rs::execute_with_heartbeat`. The helper owns the entire lifecycle of one child: spawn with `OMAKURE_RUN_ID` injected, refresh the SQLite lease every 250 ms via `runs::heartbeat`, kill on `--timeout`, react to mid-execution cancel, drain stdout/stderr through bounded mpsc channels (so an orphaned grandchild holding the pipe write end open cannot deadlock the executor).
- **Structured Trace Stream:** `src/cli/trace.rs` implements `omakure trace`, called from inside scripts via subprocess. The verb reads `OMAKURE_RUN_ID` from its environment, validates `--data` as JSON, and inserts one row in the `run_traces` table with a monotonic per-run `sequence` (computed inside a SQLite transaction so concurrent calls cannot collide). The reader is `omakure history traces <run_id>`, exposing snapshot semantics with `--level` and `--since-sequence` filters for incremental fetches. `PRAGMA foreign_keys = ON` is set on every `runs.rs` connection so deleting a run cascades to its traces.
- **Theme System:** TOML-based themes with built-in defaults compiled via `include_str!`. Supports user-defined themes in the config directory.
- **Lua Widget Extension:** Directories can contain `index.lua` files that return custom widget data rendered in the TUI.
- **History Dashboards View:** The History screen exposes two views toggled with `Tab` from `src/adapters/tui/state/history.rs::HistoryView`. `List` is the existing table-of-runs; `Dashboards` (in `src/adapters/tui/widgets/dashboards.rs`) renders a `BarChart` of runs by state, a `Sparkline` of runs per day over the last 14 days, and a per-script panel with a Canvas pie chart by state plus a duration sparkline (`avg/p50/p95`) for the row currently highlighted in `List`. All aggregations are pure functions over `app.history.entries`, so no extra SQL is issued and the dashboard always agrees with the visible list. Pressing `e`/`Enter` inside `Dashboards` expands the per-script panel into a full-view layout via `DashboardLayout::ExpandedPerScript`. The pie chart falls back to a horizontal stacked "ribbon" bar when the panel is too narrow for a legible Canvas pie.
- **Themed Loading Spinners:** A single `App.tick: u64` counter is incremented once per main-loop iteration in `src/adapters/tui/mod.rs` and consumed by `src/adapters/tui/widgets/spinner.rs::spinner_span` to drive braille frames sourced from the `rattles` crate. Two themes are wired: `Scan` lights up the search-index "Indexing" banner, `Sand` covers the bootstrap loading screen, the Lua widget loader placeholder, and the foreground `Running` screen. Spinner color tracks `theme.text_secondary()` so all built-in themes stay visually coherent.
- **Global Workspace vs. Session Scripts Root:** The `Workspace` type tracks two distinct path anchors. The **global root** owns all persisted Omakure state (`.history/`, `.omaken/`, `.omaken/envs/`, the SQLite search index, `omakure.toml`) and is the only path `Workspace::ensure_layout()` ever creates files in. The **scripts root** is the directory the TUI browses for the current session. By default both anchors point at the same directory; `omakure <PATH>` overrides only the scripts root, and the global root remains anchored to the platform default. History entries are keyed by absolute canonical script paths so runs are addressable across both invocation modes; the in-session history view is filtered by `history_belongs_to_scripts_root` against the active scripts root, with legacy relative entries resolved against the global root for backward compatibility. A per-directory `<scripts-root>/omakure.conf` is treated as a read-only session env override (parsed via `parse_env_defaults`) only when the TUI was launched with a positional path; the override never writes to `.omaken/envs/active` or copies into `.omaken/envs/`.

## Infrastructure

- **CI/CD:** GitHub Actions with two workflows:
  - `release.yml`: Cross-platform build matrix (Linux x86_64, macOS x86_64, Windows x86_64). Builds release binaries, packages as tar.gz/zip, uploads to GitHub Releases.
  - `auto-release.yml`: Triggered on PR merge to `main`. Auto-increments patch version, updates `Cargo.toml`, creates git tag, triggers `release.yml`.
- **Build Targets:** `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`.
- **Task Runner:** mise.toml configured with tasks: `tui`, `build`, `test`, `lint`, `install`.
- **Two Binaries:** `omakure` (main TUI + CLI) and `omakure-installer` (standalone installer).
- **License:** AGPL-3.0-only.
