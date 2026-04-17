# AGENTS.md

Agent guidelines for working in the omakure codebase.

## Project Overview

Omakure is a Rust TUI application for navigating and executing automation scripts. Users organize scripts in folders, and Omakure provides navigation, schema-driven input forms, execution history, and optional Lua widgets.

**Key concepts:**
- **Workspace**: Root directory containing scripts (default: `~/Documents/omakure-scripts`)
- **Schema**: JSON metadata scripts embed in a commented block between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`
- **Omaken**: Hidden `.omaken/` folder for config, environments, and widgets
- **Environments**: `.conf` files in `.omaken/envs/` providing default field values

## Commands

### Build & Run

```bash
cargo build               # Build debug
cargo build --release     # Build release
cargo run                 # Run TUI (uses repo scripts/ as workspace in debug mode)
cargo test                # Run all tests (~730 inline + 6 integration)

# Task runner shortcuts (see mise.toml)
mise run dev              # Build, start scheduler daemon, tail log, open TUI
mise run daemon-start     # Background `omakure serve -d`
mise run daemon-stop      # `omakure serve --stop`
mise run daemon-log       # Tail .omaken/daemon.log
mise run coverage         # cargo tarpaulin HTML report
mise run lint             # clippy -D warnings + fmt --check
```

### Environment Variables

- `OMAKURE_SCRIPTS_DIR` is the preferred scripts directory override
- Legacy overrides are accepted: `OVERTURE_SCRIPTS_DIR`, `CLOUD_MGMT_SCRIPTS_DIR`
- Update command also reads `OMAKURE_REPO`/`REPO` plus legacy `OVERTURE_REPO`/`CLOUD_MGMT_REPO` and `VERSION`

Debug builds automatically use `scripts/` in the repo if it exists. Otherwise resolution order: env overrides above, `~/Documents/omakure-scripts` if present, legacy `~/Documents/overture-scripts`/`cloud-mgmt-scripts`, then `~/Documents/omakure-scripts` fallback.

### Release

Releases are built via GitHub Actions (`.github/workflows/release.yml`) triggered by version tags (`v*`). Targets:
- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `x86_64-pc-windows-msvc`

The `update` command defaults to GitHub repo `This-Is-NPC/omakure` (overridable via flags/env) and copies missing scripts from the tagged release into the workspace scripts directory.

## Architecture

Ports-and-adapters (hexagonal) architecture:

```
src/
├── cli/                      # CLI subcommands
│   ├── mod.rs
│   ├── args.rs               # clap definitions for the whole CLI (long_about docs)
│   ├── json.rs               # Single JSON envelope helper + stable error codes
│   ├── run.rs                # `omakure run` — headless execution
│   ├── queue.rs              # `omakure queue add|cancel|dead-letter|worker|stats`
│   ├── serve.rs              # `omakure serve` — cron scheduler daemon
│   ├── serve_autostart.rs    # systemd user service install/uninstall/status (Linux)
│   ├── trace.rs              # `omakure trace` — in-script structured event writer
│   ├── history.rs            # `omakure history list|show|tail|stats|traces`
│   ├── describe.rs           # `omakure describe <script>` (AI verb)
│   ├── search.rs             # `omakure search <query>` (AI verb, --tag AND filter)
│   ├── list.rs               # `omakure scripts` listing (+ --json, --tag)
│   ├── init.rs               # Script template generation (+ --schema-json/--body-stdin/--force)
│   ├── help_ai.rs            # `omakure help-ai` — capability discovery from clap metadata
│   ├── config.rs             # `omakure config` — resolved paths (+ --json)
│   ├── doctor.rs             # `omakure doctor` — runtime + schema checks
│   ├── theme.rs              # `omakure theme list|set|preview|path`
│   ├── omaken.rs             # `omakure list`/`omakure install` — Omaken flavor management
│   ├── update.rs             # Self-update from GitHub releases
│   └── uninstall.rs          # Binary removal (+ optional --scripts wipe)
├── domain/                   # Core types, no I/O
│   ├── mod.rs
│   ├── schema.rs             # Schema, Field, Schedule, OutputField, QueueSpec
│   ├── parsing.rs            # Extract schema block + JSON parse
│   ├── validation.rs         # Field input normalization
│   └── schedule.rs           # Cron normalize/parse + next_fire_after
├── ports/
│   ├── mod.rs                # ScriptRepository, ScriptRunner
│   └── environment.rs        # EnvironmentRepository, EnvironmentConfig
├── use_cases/
│   ├── mod.rs                # ScriptService orchestration
│   └── environment.rs        # EnvironmentService
├── adapters/
│   ├── environments.rs       # Environment config loading + sensitive-key masking
│   ├── omarchy.rs            # Omarchy theme detection + import
│   ├── script_runner.rs      # MultiScriptRunner (bash/ps1/py command builder)
│   ├── system_checks.rs      # Runtime dependency checks (git, bash, jq, pwsh, python)
│   ├── workspace_repository.rs  # Filesystem-backed ScriptRepository
│   └── tui/                  # Terminal UI (ratatui)
│       ├── app.rs            # App state + Screen enum + in-place schedule toggle
│       ├── events.rs         # Keyboard event handlers
│       ├── mod.rs
│       ├── theme.rs          # Colors and styles
│       ├── ui.rs             # Render dispatch
│       ├── state/            # Per-screen state (navigation, search, history, …)
│       └── widgets/          # Stateless renderers incl. schedules.rs, activity_grid.rs,
│                              # dashboards.rs, history.rs, scripts.rs, schema.rs, …
├── error.rs                  # AppError / SchemaError / ScriptError / EnvironmentError
├── util.rs                   # Shared helpers (ps_quote, TempDirGuard, set_executable_permissions)
├── workspace.rs              # Workspace layout (global root vs. session scripts root)
├── runs.rs                   # SQLite run state machine + structured trace storage
├── run_executor.rs           # Shared child-process lifecycle (run + worker + scheduler)
├── search_index.rs           # SQLite-backed script search + background rebuild
├── lua_widget.rs             # Lua 5.4 widget loader for directory widgets
├── runtime.rs                # Script kind detection and command building
├── theme_config.rs           # Global ~/.config/omakure/config.toml handling
├── installer.rs              # Standalone installer binary (omakure-installer)
├── app_meta.rs               # App version + repo URL constants
└── main.rs                   # CLI routing, scripts-dir resolution, TUI entry
```

> **Removed in this release:** `src/history.rs` and the `HistoryEntry`
> JSON-file format. The TUI history screen and the headless `run`
> command both write through `runs.rs` directly. There is no shim and
> no compatibility re-export. The destructive upgrade behavior (legacy
> `.history/*.json` files are deleted on first launch) is documented
> in `.docs/ai-interface.md`.

### Key Files

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI entry, command routing, scripts dir resolution (with legacy env fallbacks), TUI launch |
| `src/cli/mod.rs` | CLI module exports and `wants_help` helper |
| `src/cli/run.rs` | Headless script execution; routes through the state machine via `run_executor::execute_with_heartbeat` |
| `src/cli/queue.rs` | `omakure queue add | cancel | dead-letter | worker | stats` — producers + worker daemon |
| `src/cli/trace.rs` | `omakure trace` — script-side structured trace writer (reads `OMAKURE_RUN_ID` from env) |
| `src/cli/json.rs` | Single JSON envelope writer (`{ ok, data, error, schema_version }`) and stable error codes |
| `src/cli/describe.rs` | `omakure describe <script>` — full schema for one script |
| `src/cli/search.rs` | `omakure search <query>` — surfaces the SQLite script index (+ `--tag` AND filter) |
| `src/cli/history.rs` | `omakure history list/show/tail/stats/traces` — query the run log; `--state` / `--state-set` filters |
| `src/cli/help_ai.rs` | `omakure help-ai` — single-call AI capability discovery generated from clap |
| `src/runs.rs` | SQLite-backed run state machine + structured trace storage; exposes `RunState`, `enqueue`, `start_inline`, `claim_next`, `complete`, `fail`, `cancel`, `time_out`, `dead_letter`, `heartbeat`, `insert_trace`, `query_traces`, `stats` |
| `src/run_executor.rs` | Shared execution helper used by both `omakure run` and the worker; spawns the child with `OMAKURE_RUN_ID`, heartbeats the lease, kills on `--timeout`, reacts to mid-execution cancel |
| `src/cli/update.rs` | Self-update via GitHub Releases and script sync into scripts dir |
| `src/cli/omaken.rs` | Omaken flavor listing/install (`list`/`install` commands) |
| `src/cli/list.rs` | `scripts` subcommand: recursive script listing |
| `src/adapters/environments.rs` | Environment config loading and active env management |
| `src/adapters/system_checks.rs` | Runtime dependency availability checks |
| `src/adapters/tui/app.rs` | App struct with all TUI state, Screen enum |
| `src/adapters/tui/events.rs` | Keyboard handlers by screen |
| `src/util.rs` | `set_executable_permissions()`, `ps_quote()`, `TempDirGuard` |
| `src/error.rs` | `AppError` enum and `AppResult` type alias |
| `src/installer.rs` | Windows installer entrypoint that copies the binary and patches PATH |

### TUI Screens

Defined in `src/adapters/tui/app.rs`:

```rust
pub(crate) enum Screen {
    ScriptSelect,   // Main list navigation
    Search,         // Ctrl+S fuzzy search
    Environments,   // Alt+E environment selector
    FieldInput,     // Script parameter form
    History,        // H key — execution history (List / Dashboards / Activity grid views)
    Schedules,      // c key — cron schedules; Space toggles Enabled in place
    Running,        // Script executing
    RunResult,      // Execution output display
    Error,          // Error display
}
```

## Code Conventions

### Naming

- **Structs**: PascalCase (`WorkspaceEntry`, `ScriptService`)
- **Functions**: snake_case (`load_schema`, `run_script`)
- **Modules**: snake_case (`script_runner`, `workspace_repository`)
- **Constants**: SCREAMING_SNAKE_CASE (`BRAND_GRADIENT_START` in `tui/theme.rs`, `SCHEMA_VERSION` in `cli/json.rs`)

### Visibility

- Public API uses `pub`
- TUI internals use `pub(crate)` for cross-module access within adapters
- Helper functions stay private

### Error Handling

- Functions return `Result<T, Box<dyn Error>>` or `io::Result<T>`
- Use `?` for propagation
- Custom `AppError` type available in `src/error.rs` for gradual migration
- Custom error messages via `.into()` or `format!("...")`

### Serde Conventions

Schema JSON uses PascalCase field names:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Schema {
    pub name: String,
    pub description: Option<String>,
    pub fields: Vec<Field>,
}
```

### Path Handling

- Use `PathBuf` for owned paths, `&Path` for borrowed
- Cross-platform: handle both `/` and `\` separators
- Windows Documents folder resolved via registry in `main.rs`

## Dependencies

| Crate | Purpose |
|-------|---------|
| `ratatui` | Terminal UI framework |
| `crossterm` | Cross-platform terminal manipulation |
| `serde` / `serde_json` | JSON serialization |
| `mlua` | Lua 5.4 embedding for widgets |
| `rusqlite` | SQLite for search index |
| `winreg` | Windows registry access (Windows only) |

## Workspace Layout

Scripts directory structure:

```
~/Documents/omakure-scripts/
├── omakure.toml              # Workspace version config
├── .omaken/
│   ├── envs/
│   │   ├── active            # Current env name
│   │   ├── dev.conf          # KEY=value defaults
│   │   └── env_template.conf
│   ├── daemon.pid            # `omakure serve` PID lock (created by the daemon)
│   ├── daemon.log            # Structured scheduler log (RFC3339 lines)
│   └── <folder>/
│       └── index.lua         # Optional Lua widget
├── .history/
│   ├── runs.sqlite           # Run state machine + structured traces
│   └── search-index.sqlite   # Script search DB
└── <scripts and folders>
```

## Script Schema Format

Scripts embed metadata in a commented block:

```text
# OMAKURE_SCHEMA_START
# {
#   "Name": "my_script",
#   "Description": "What it does",
#   "Tags": ["optional", "tags"],
#   "Fields": [
#     {
#       "Name": "target",
#       "Prompt": "Enter target",
#       "Type": "string",
#       "Order": 1,
#       "Required": true,
#       "Arg": "--target",
#       "Default": "default_value",
#       "Choices": ["option1", "option2"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END
```

**Comment prefixes by extension**:
- `.bash`/`.sh`: `#`
- `.ps1`: `#` or `;`
- `.py`: `#`

**Field types**: `string`, `number`, `bool`/`boolean`

## Lua Widgets

Folders can have `index.lua` returning widget data:

```lua
return {
  title = "Widget Title",
  lines = { "Line 1", "Line 2" }
}
```

Widgets load asynchronously in background threads.

## Adding a New CLI Subcommand

Commands are driven by clap (derive). Steps:

1. Declare the subcommand in `src/cli/args.rs`:
   - Add a variant to the `Commands` enum with `/// Short about` and an optional `long_about` via doc comment (blank line after the short line).
   - If the subcommand takes flags, add a `#[derive(Args, Debug)]` struct and attach it to the variant (e.g. `MyCmd(MyCmdArgs)`).
2. Create `src/cli/mycmd.rs` with `pub fn run(scripts_dir: PathBuf, args: MyCmdArgs, json_output: bool) -> Result<(), Box<dyn Error>>`.
3. Register the module in `src/cli/mod.rs` (`pub mod mycmd;`).
4. Wire the dispatch arm in `src/main.rs` under `match cli.command { ... }`.
5. If the subcommand emits JSON, honor the `json_output` flag and use helpers in `src/cli/json.rs` (`json::print_ok(...)` / `json::print_err(code, msg)`). Use a stable code from `cli::json::codes`.
6. If it targets an AI agent, add the verb name to `AI_VERBS` in `src/cli/help_ai.rs` so it surfaces in `omakure help-ai`.

Reference patterns: `src/cli/serve.rs` (long-running daemon, lock file, signal handling), `src/cli/run.rs` (synchronous fast path), `src/cli/history.rs` (subcommands-within-subcommands with JSON envelope).

## Adding a New TUI Screen

1. Add variant to `Screen` enum in `src/adapters/tui/app.rs`
2. Add state fields to `App` struct if needed
3. Add handler function in `src/adapters/tui/events.rs`
4. Add render function in `src/adapters/tui/ui.rs` or new widget file
5. Wire handler in `handle_key_event()` match
6. Wire render in `render_ui()` match

## Common Patterns

### Loading Scripts

```rust
let repo = FsWorkspaceRepository::new(scripts_dir);
let entries = repo.list_entries(&current_dir)?;
let schema = repo.read_schema(&script_path)?;
```

### Running Scripts

```rust
let runner = MultiScriptRunner::new();
let output = runner.run(&script_path, &args)?;
```

### History Recording

```rust
let conn = runs::open(&workspace)?;
let row = RunRow { /* run_id, script_path, actor, ... */ };
let _ = runs::insert_run(&conn, &row);
```

### Using Shared Utilities

```rust
use crate::util::{set_executable_permissions, ps_quote, TempDirGuard};

// Set Unix permissions
set_executable_permissions(&path)?;

// Quote for PowerShell
let quoted = ps_quote("path with 'quotes'");

// Auto-cleanup temp dir
let temp_dir = TempDirGuard::new(path);
// Dir removed when temp_dir goes out of scope
```

## Testing

Run all tests:
```bash
cargo test
```

Tests are inline (`#[cfg(test)] mod tests`) in most source files. Highlights:

- `src/domain/schema.rs`, `src/domain/parsing.rs`, `src/domain/validation.rs`, `src/domain/schedule.rs` — schema + cron parsing, input normalization
- `src/runs.rs` — state machine transitions, legacy json cleanup, rebuild-legacy-schema, traces, queries (45+ tests)
- `src/cli/serve.rs` — `scheduler_tick`, lock acquire/reclaim, overlap skip
- `src/cli/serve_autostart.rs` — systemd unit naming + rendering
- `src/cli/queue.rs`, `src/cli/history.rs`, `src/cli/init.rs`, `src/cli/run.rs` — subcommand surface
- `src/adapters/tui/app.rs` — state machine, in-place schedule toggle
- `src/adapters/tui/widgets/activity_grid.rs`, `widgets/schedules.rs`, `widgets/dashboards.rs` — rendering
- `tests/cli_positional_path.rs` — 6 integration tests for the positional-path mode

Coverage is measured via `mise run coverage` (cargo-tarpaulin). Full test run: `mise run test`.

## Gotchas

1. **Schema JSON is PascalCase** - Field names like `Name`, `Description`, not `name`, `description`

2. **Schema blocks** - Scripts must include a commented JSON schema block between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`

3. **Scripts dir resolution** - Order: env overrides (including legacy `OVERTURE_`/`CLOUD_MGMT_`), repo `scripts/` in debug builds, `~/Documents/omakure-scripts` if present, legacy `overture-scripts`/`cloud-mgmt-scripts`, then `~/Documents/omakure-scripts` fallback (Windows Documents path comes from the registry).

4. **Omaken vs scripts commands** - `list`/`install` manage `.omaken` flavors; use `scripts` command to enumerate runnable scripts.

5. **Widget loading is async** - `start_widget_load()` spawns a thread, `poll_widget_load()` checks completion

6. **Search index background rebuild** - `SearchIndex::start_background_rebuild()` runs in a background thread on startup

7. **Script types by extension** - `.bash`/`.sh` → bash, `.ps1` → PowerShell (`pwsh` on non-Windows), `.py` → Python3 (determined in `runtime.rs`); schema blocks must use extension-specific comment prefixes

8. **Update dependencies** - `update` uses curl/wget/PowerShell to fetch releases and copies missing scripts from the tag into the workspace; ensure those tools are present.

## Documentation

Detailed docs in `.docs/`:

- `architecture.md` — tech stack, patterns, code metrics, infrastructure
- `requirements.md` — implemented FRs, NFRs, and business rules (file-referenced)
- `ai-interface.md` — JSON envelope contract and AI-facing verbs
- `usage.md` — CLI usage (incl. `omakure serve` scheduler)
- `installation.md` — install/update/uninstall flows
- `workspace.md` — workspace structure (global vs. session)
- `scripts-path.md` — scripts directory precedence
- `environments.md` — environment defaults system
- `how-to-create-a-script.md` — script template + schema guide (incl. `Schedule` block)
- `how-it-works.md` — high-level overview
- `lua-widgets.md` — widget format
- `development.md` — dev workflow + mise tasks
- `release-artifacts.md` — release archive naming

## File References

When making changes, key files to consider:

| Change Type | Files |
|------------|-------|
| CLI subcommands | `src/main.rs`, `src/cli/mod.rs`, new file in `src/cli/` |
| TUI behavior | `src/adapters/tui/app.rs`, `events.rs`, `ui.rs` |
| Script execution | `src/adapters/script_runner.rs`, `src/runtime.rs` |
| Schema parsing | `src/domain/mod.rs` |
| Workspace layout | `src/workspace.rs` |
| History | `src/runs.rs` (SQLite-backed); TUI rendering in `src/adapters/tui/widgets/history.rs` |
| Search | `src/search_index.rs` |
| Themes/colors | `src/adapters/tui/theme.rs` |
| Shared utilities | `src/util.rs` |
| Error handling | `src/error.rs` |
| System checks | `src/adapters/system_checks.rs` |
| Environment config | `src/adapters/environments.rs` |
