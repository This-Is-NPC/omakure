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
| FR-009 | Execution history recorded in `<workspace>/.history/runs.sqlite` with `run_id`, `script_path`, `actor`, `reason`, args, exit code, stdout, stderr, start/end timestamps, duration, and parent run id | `src/runs.rs`, `src/cli/run.rs`, `src/adapters/tui/mod.rs` |
| FR-010 | History browsing in TUI with output preview, scroll, and `Actor` column, sourced from `runs.sqlite` | `src/adapters/tui/app.rs`, `src/adapters/tui/widgets/history.rs`, `src/adapters/tui/state/history.rs` |
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
| FR-031 | Optional positional path argument launches TUI against any directory as a session-only scripts root, leaving global state untouched | `src/cli/args.rs`, `src/main.rs` (resolve_scripts_root, run_tui) |
| FR-032 | History entries are recorded with the absolute canonical path of the executed script; legacy relative entries continue to load and are filtered against the global workspace root | `src/history.rs` (script_path), `src/adapters/tui/app.rs` (history_belongs_to_scripts_root) |
| FR-033 | `<scripts-root>/omakure.conf` becomes the session-active environment when the TUI is launched with a positional path; absent file falls back to the globally active env; parser tolerates malformed lines silently and any I/O failure surfaces a non-fatal error while the TUI keeps launching | `src/adapters/tui/app.rs` (load_env_config, load_session_env_config), `src/adapters/environments.rs` (parse_env_defaults), `src/workspace.rs` (has_scripts_root_override) |
| FR-034 | Global `--json` flag emits a uniform `{ ok, data, error, schema_version: "1" }` envelope for `scripts`, `describe`, `search`, `run`, `init`, `history`, `config`, and `help-ai` | `src/cli/json.rs`, `src/cli/args.rs`, `src/main.rs` |
| FR-035 | `omakure describe <script>` returns the full parsed schema (name, description, tags, fields with type/required/arg/default/choices/order) plus the resolved absolute script path; missing scripts return `error.code = "not_found"`, malformed schemas return `error.code = "schema_invalid"` | `src/cli/describe.rs` |
| FR-036 | `omakure search <query>` surfaces the SQLite-backed script index from the CLI (previously TUI-only), returning the same per-script shape as `scripts --json` | `src/cli/search.rs`, `src/search_index.rs` |
| FR-037 | `omakure history list/show/tail` queries `.history/runs.sqlite` with filters `--script`, `--actor`, `--since`, `--until`, `--success`, `--failure`, `--limit`; orders by `started_at DESC`; `show <run_id>` returns the full row including stdout/stderr; unknown ids return `error.code = "not_found"` | `src/cli/history.rs`, `src/runs.rs` |
| FR-038 | `omakure help-ai` emits a single JSON capability payload (verbs, flags, error codes, envelope shape, data shapes) generated by walking `Cli::command()` so it cannot drift from `--help` | `src/cli/help_ai.rs` |
| FR-039 | `omakure run` accepts `--actor`, `--reason`, `--run-id`, `--parent-run-id`, `--no-prompt`; `--json` implies `--no-prompt`; `--no-prompt` is a pre-flight check that fails with `missing_required_field` and writes no history row when a required field has no `--<arg>` on the command line | `src/cli/run.rs`, `src/cli/args.rs` |
| FR-040 | `omakure init` accepts `--schema-json '<json>|@file'`, `--body-stdin`, and `--force`; the supplied schema is validated before the script is written; `script_exists` is returned for existing files without `--force`; `schema_invalid` is returned for malformed schemas | `src/cli/init.rs`, `src/cli/args.rs` |
| FR-041 | First-launch cleanup: every top-level `*.json` file in `<workspace>/.history/` is deleted on the first call to `runs::open` against a workspace; subdirectories, `runs.sqlite`, `search-index.sqlite`, and `.omaken/` are left untouched; subsequent opens are natural no-ops | `src/runs.rs` (`cleanup_legacy_json_files`) |
| FR-042 | Run state machine on `runs.sqlite` with seven final states (`queued`, `running`, `completed`, `failed`, `cancelled`, `timed_out`, `dead_letter`) and a closed transitions matrix; illegal transitions return `error.code = "invalid_argument"`. v0.1-shaped tables (no `state` column) are detected and rebuilt destructively on first open | `src/runs.rs` (`RunState`, `init_schema`, `rebuild_legacy_schema_if_needed`, `enqueue`, `start_inline`, `claim_next`, `complete`, `fail`, `cancel`, `time_out`, `dead_letter`, `heartbeat`) |
| FR-043 | Single shared execution code path used by both `omakure run` (synchronous fast path) and `omakure queue worker` (daemon); spawns the child with `OMAKURE_RUN_ID` injected, refreshes the SQLite lease via `runs::heartbeat`, reacts to mid-execution cancel and per-row `--timeout`, drains stdout/stderr through bounded mpsc channels | `src/run_executor.rs` (`execute_with_heartbeat`), `src/adapters/script_runner.rs` (`MultiScriptRunner::build_command`) |
| FR-044 | Producer verbs `omakure queue add | cancel | dead-letter | stats` writing through the state machine. `add` resolves the script and inserts a `queued` row with optional `--actor`/`--reason`/`--priority`/`--timeout`/`--parent-run-id`; `cancel` flips queued rows instantly and lets the worker kill running rows on the next heartbeat; `dead-letter` only succeeds against `failed`/`timed_out` | `src/cli/queue.rs`, `src/runs.rs` |
| FR-045 | Long-running `omakure queue worker --concurrency N` daemon claims jobs atomically via `UPDATE … RETURNING`, runs them through the shared executor, and finishes its in-flight jobs cleanly on SIGINT/SIGTERM (handled via `signal-hook`). Crashed workers' jobs are reclaimed automatically once `lease_until` (60 s heartbeat) expires | `src/cli/queue.rs` (`worker_loop`, `install_signal_handlers`), `src/runs.rs` (`claim_next`) |
| FR-046 | Structured trace stream: `omakure trace` writer reads `OMAKURE_RUN_ID` from env, validates `--level` and `--data` (must parse as JSON), inserts a row in `run_traces` with monotonic per-run `sequence` (computed inside a SQLite transaction). When `OMAKURE_RUN_ID` is unset the verb is a silent no-op so scripts remain testable standalone | `src/cli/trace.rs`, `src/runs.rs` (`insert_trace`) |
| FR-047 | `omakure history traces <run_id>` reader returns trace rows ordered by `sequence ASC`, with `--level` (minimum-level filter) and `--since-sequence` (incremental fetch) support; `PRAGMA foreign_keys = ON` cascades trace deletes when a run is removed | `src/cli/history.rs` (`traces`), `src/runs.rs` (`query_traces`, `open_connection` PRAGMA) |
| FR-048 | `omakure scripts` and `omakure search` accept a repeatable `--tag` flag with case-sensitive AND semantics against the embedded schema's `Tags` field | `src/cli/list.rs` (`matches_all_tags`), `src/cli/search.rs` |
| FR-049 | `omakure history list` accepts `--state` (repeatable) and `--state-set` (`in_flight`/`terminal`/`all`), mutually exclusive; default when neither is set is the terminal set so v0.1 callers see no behavior change | `src/cli/history.rs` (`resolve_state_filter`), `src/runs.rs` (`RunFilters::default`) |
| FR-050 | `omakure history stats` returns counts per state and per actor in a single envelope (the same data as `queue stats`, exposed under the visibility surface for fleet dashboards) | `src/cli/history.rs` (`stats`), `src/runs.rs` (`stats`) |
| FR-051 | TUI history screen renders a per-state colored State column and surfaces in-flight rows (`queued`/`running`) at the top of the list, ordered by `enqueued_at`/`started_at` DESC | `src/adapters/tui/widgets/history.rs` (`state_color`), `src/runs.rs` (`query_runs` ordering) |

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
