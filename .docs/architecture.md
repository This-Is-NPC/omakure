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
| winreg | 0.52 | Windows registry access for Documents path |

## Project Structure

```
src/
├── main.rs                  # Entry point, CLI dispatch, TUI bootstrap
├── installer.rs             # Standalone binary for omakure-installer
├── app_meta.rs              # App version and repo URL constants
├── error.rs                 # Centralized error types (AppError, SchemaError, ScriptError, EnvironmentError)
├── runs.rs                  # SQLite-backed run history (replaces the deleted history.rs)
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
│           ├── field_input.rs
│           ├── environment.rs
│           ├── envs.rs
│           ├── running.rs
│           ├── run_result.rs
│           ├── error.rs
│           ├── loading.rs
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
    ├── history.rs           # `omakure history list|show|tail` AI verb
    ├── help_ai.rs           # `omakure help-ai` capability discovery
    ├── list.rs              # `omakure scripts` list available scripts (+ --json)
    ├── init.rs              # `omakure init` create script template (+ --schema-json/--body-stdin/--force)
    ├── config.rs            # `omakure config` show resolved paths (+ --json)
    ├── omaken.rs            # `omakure list/install` flavor management
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
- **SQLite Run Log:** Run history is persisted in `<workspace>/.history/runs.sqlite` via `src/runs.rs`, which exposes `RunRow`/`RunFilters` and is the only writer of the run log. The TUI history screen reads `RunRow` directly. On first open against a workspace, every top-level legacy `*.json` file in `history_dir()` is unlinked — there is no migration path from the previous JSON-file format.
- **Theme System:** TOML-based themes with built-in defaults compiled via `include_str!`. Supports user-defined themes in the config directory.
- **Lua Widget Extension:** Directories can contain `index.lua` files that return custom widget data rendered in the TUI.
- **Global Workspace vs. Session Scripts Root:** The `Workspace` type tracks two distinct path anchors. The **global root** owns all persisted Omakure state (`.history/`, `.omaken/`, `.omaken/envs/`, the SQLite search index, `omakure.toml`) and is the only path `Workspace::ensure_layout()` ever creates files in. The **scripts root** is the directory the TUI browses for the current session. By default both anchors point at the same directory; `omakure <PATH>` overrides only the scripts root, and the global root remains anchored to the platform default. History entries are keyed by absolute canonical script paths so runs are addressable across both invocation modes; the in-session history view is filtered by `history_belongs_to_scripts_root` against the active scripts root, with legacy relative entries resolved against the global root for backward compatibility. A per-directory `<scripts-root>/omakure.conf` is treated as a read-only session env override (parsed via `parse_env_defaults`) only when the TUI was launched with a positional path; the override never writes to `.omaken/envs/active` or copies into `.omaken/envs/`.

## Infrastructure

- **CI/CD:** GitHub Actions with two workflows:
  - `release.yml`: Cross-platform build matrix (Linux x86_64, macOS x86_64, Windows x86_64). Builds release binaries, packages as tar.gz/zip, uploads to GitHub Releases.
  - `auto-release.yml`: Triggered on PR merge to `main`. Auto-increments patch version, updates `Cargo.toml`, creates git tag, triggers `release.yml`.
- **Build Targets:** `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`.
- **Task Runner:** mise.toml configured with tasks: `tui`, `build`, `test`, `lint`, `install`.
- **Two Binaries:** `omakure` (main TUI + CLI) and `omakure-installer` (standalone installer).
- **License:** AGPL-3.0-only.
