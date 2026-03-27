# Implemented Requirements

## Functional Requirements

| ID | Description | Source files |
|----|------------|-------------|
| FR-001 | Interactive TUI for browsing and selecting scripts from a workspace directory | `src/adapters/tui/app.rs`, `src/adapters/tui/widgets/scripts.rs` |
| FR-002 | Hierarchical directory navigation with parent traversal in script browser | `src/adapters/tui/app.rs` (enter_selected, navigate_up) |
| FR-003 | Script schema parsing from embedded JSON blocks in comment sections (OMAKURE_SCHEMA_START/END) | `src/domain/parsing.rs`, `src/adapters/workspace_repository.rs` |
| FR-004 | Dynamic form generation from schema fields with type validation (string, number, boolean) | `src/domain/validation.rs`, `src/adapters/tui/app.rs` (submit_form) |
| FR-005 | Choice-constrained fields with validation against allowed values | `src/domain/validation.rs` |
| FR-006 | Default values for fields, overridable by environment configuration | `src/adapters/tui/app.rs` (build_field_inputs) |
| FR-007 | Multi-runtime script execution: Bash (.bash/.sh), PowerShell (.ps1), Python (.py) | `src/runtime.rs`, `src/adapters/script_runner.rs` |
| FR-008 | Runtime dependency checking before execution (git, bash, jq, python, pwsh) | `src/adapters/script_runner.rs`, `src/adapters/system_checks.rs` |
| FR-009 | Execution history recording as JSON files with timestamp, args, stdout, stderr, exit code | `src/history.rs` |
| FR-010 | History browsing in TUI with output preview and scroll | `src/adapters/tui/app.rs`, `src/adapters/tui/widgets/history.rs` |
| FR-011 | Full-text search index backed by SQLite with background rebuild | `src/search_index.rs` |
| FR-012 | Search screen with live query filtering and script detail preview | `src/adapters/tui/app.rs` (enter_search, refresh_search_results), `src/adapters/tui/widgets/search.rs` |
| FR-013 | Environment management: list, activate, deactivate env files | `src/adapters/environments.rs`, `src/use_cases/environment.rs` |
| FR-014 | Environment preview with sensitive value masking (password, secret, token, key, api, private, cred) | `src/adapters/environments.rs` (is_sensitive_key) |
| FR-015 | CLI `run` command for headless script execution | `src/cli/run.rs` |
| FR-016 | CLI `doctor` command for runtime health checks | `src/cli/doctor.rs` |
| FR-017 | CLI `init` command for script template creation | `src/cli/init.rs` |
| FR-018 | CLI `config` command to display resolved paths and environment | `src/cli/config.rs` |
| FR-019 | CLI `scripts` command to list available scripts | `src/cli/list.rs` |
| FR-020 | Omaken flavor system: list and install script collections from git repositories | `src/cli/omaken.rs` |
| FR-021 | Theme system with TOML-based themes (5 built-in: default, dracula, catppuccin-mocha, nord, solarized-dark) | `src/adapters/tui/theme.rs`, `themes/` |
| FR-022 | Theme management CLI: list, set, preview themes | `src/cli/theme.rs` |
| FR-023 | Shell completion generation (bash, zsh, fish, powershell) | `src/cli/args.rs`, `src/main.rs` (generate_completions) |
| FR-024 | Self-update from GitHub releases | `src/cli/update.rs` |
| FR-025 | Self-uninstall with optional scripts directory removal | `src/cli/uninstall.rs` |
| FR-026 | Lua widget extension: custom TUI widgets via `index.lua` in directories | `src/lua_widget.rs` |
| FR-027 | Workspace auto-initialization: creates root, .omaken, .history, envs, omakure.toml | `src/workspace.rs` |
| FR-028 | Queue/Matrix execution support via schema (matrix values and named cases) | `src/domain/schema.rs` (QueueSpec, MatrixSpec, QueueCase) |
| FR-029 | Schema preview in script browser showing name, description, tags, fields, outputs, queue | `src/adapters/tui/app.rs` (update_schema_preview), `src/adapters/tui/widgets/schema.rs` |
| FR-030 | Standalone installer binary | `src/installer.rs` |

## Non-Functional Requirements

| ID | Description | Source files |
|----|------------|-------------|
| NFR-001 | Cross-platform support: Linux, macOS, Windows (conditional compilation for Windows registry and paths) | `src/main.rs`, `src/runtime.rs`, `Cargo.toml` (winreg) |
| NFR-002 | Background search indexing with non-blocking status polling via channels | `src/search_index.rs` (start_background_rebuild), `src/adapters/tui/app.rs` (poll_widget_load) |
| NFR-003 | SQLite WAL mode with busy timeout for concurrent access | `src/search_index.rs` (open_connection) |
| NFR-004 | Graceful terminal restore on TUI exit (raw mode cleanup) | `src/main.rs` (run_tui), `src/adapters/tui/mod.rs` |
| NFR-005 | Schema cache to avoid re-parsing on repeated selection | `src/adapters/tui/app.rs` (load_schema, schema_cache) |
| NFR-006 | Centralized error handling with typed error hierarchy (AppError, SchemaError, ScriptError, EnvironmentError) | `src/error.rs` |
| NFR-007 | Automated release pipeline: PR merge triggers version bump, tag, cross-platform build, and GitHub Release | `.github/workflows/auto-release.yml`, `.github/workflows/release.yml` |

## Business Rules

| ID | Rule | Source files |
|----|------|-------------|
| BR-001 | Hidden directories `.history` and `.git` are excluded from script listing; `.omaken/envs/` is also skipped | `src/adapters/workspace_repository.rs` (should_skip_dir) |
| BR-002 | Only files with extensions `.bash`, `.sh`, `.ps1`, `.py` are recognized as scripts | `src/runtime.rs` (script_extensions, script_kind) |
| BR-003 | Boolean inputs accept: true/t/yes/y/1 and false/f/no/n/0 (case-insensitive) | `src/domain/validation.rs` (parse_bool) |
| BR-004 | Environment variable keys containing password, secret, token, key, api, private, or cred are masked as `***` in preview | `src/adapters/environments.rs` (is_sensitive_key) |
| BR-005 | Scripts directory resolution priority: CLI flag > OMAKURE_SCRIPTS_DIR > OVERTURE_SCRIPTS_DIR > CLOUD_MGMT_SCRIPTS_DIR > dev `scripts/` (debug only) > `~/Documents/omakure-scripts` > legacy dirs | `src/main.rs` (scripts_dir) |
| BR-006 | History file names include timestamp, PID, and script slug (max 64 chars) for uniqueness | `src/history.rs` (history_file_name, safe_slug) |
| BR-007 | Directory entries are sorted with directories first, then scripts, both alphabetically (case-insensitive) | `src/adapters/workspace_repository.rs` (list_entries sort) |
| BR-008 | Workspace config is auto-created with current app version on first run | `src/workspace.rs` (ensure_layout, default_config) |
