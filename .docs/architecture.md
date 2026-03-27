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
├── history.rs               # Execution history: record, load, format (JSON files)
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
    ├── args.rs              # Clap argument definitions
    ├── run.rs               # `omakure run <script>`
    ├── doctor.rs            # `omakure doctor` runtime checks
    ├── list.rs              # `omakure scripts` list available scripts
    ├── init.rs              # `omakure init` create script template
    ├── config.rs            # `omakure config` show resolved paths
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
- **Theme System:** TOML-based themes with built-in defaults compiled via `include_str!`. Supports user-defined themes in the config directory.
- **Lua Widget Extension:** Directories can contain `index.lua` files that return custom widget data rendered in the TUI.

## Infrastructure

- **CI/CD:** GitHub Actions with two workflows:
  - `release.yml`: Cross-platform build matrix (Linux x86_64, macOS x86_64, Windows x86_64). Builds release binaries, packages as tar.gz/zip, uploads to GitHub Releases.
  - `auto-release.yml`: Triggered on PR merge to `main`. Auto-increments patch version, updates `Cargo.toml`, creates git tag, triggers `release.yml`.
- **Build Targets:** `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`.
- **Task Runner:** mise.toml configured with tasks: `tui`, `build`, `test`, `lint`, `install`.
- **Two Binaries:** `omakure` (main TUI + CLI) and `omakure-installer` (standalone installer).
- **License:** AGPL-3.0-only.
