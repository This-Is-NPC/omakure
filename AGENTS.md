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
cargo run                 # Run TUI (uses repo scripts/ in debug mode)
cargo test                # Run all tests (28 tests)
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
├── cli/                  # CLI subcommands
│   ├── mod.rs
│   ├── args.rs           # clap definitions for the whole CLI
│   ├── common.rs         # Shared help text/constants
│   ├── completion.rs     # Shell completion generation
│   ├── config.rs         # Config/env display (+ --json envelope)
│   ├── describe.rs       # `omakure describe <script>` (AI verb)
│   ├── doctor.rs         # Runtime checks
│   ├── help_ai.rs        # `omakure help-ai` AI capability discovery
│   ├── history.rs        # `omakure history list|show|tail` (AI verb)
│   ├── init.rs           # Script template generation (+ --schema-json/--body-stdin/--force)
│   ├── json.rs           # Single JSON envelope helper + stable error codes
│   ├── list.rs           # `omakure scripts` (+ --json envelope)
│   ├── omaken.rs         # Omaken flavor list/install commands
│   ├── run.rs            # Headless script execution (+ --actor/--reason/--json/--no-prompt)
│   ├── search.rs         # `omakure search <query>` (AI verb)
│   ├── uninstall.rs      # Binary removal
│   └── update.rs         # Self-update from GitHub
├── domain/               # Core types: Schema, Field, normalize_input
│   └── mod.rs
├── ports/                # Traits: ScriptRepository, ScriptRunner
│   └── mod.rs
├── use_cases/            # ScriptService orchestration
│   └── mod.rs
├── adapters/
│   ├── environments.rs   # Environment config loading
│   ├── script_runner.rs  # Script execution
│   ├── system_checks.rs  # Runtime dependency checks
│   ├── workspace_repository.rs  # Filesystem repository impl
│   └── tui/              # Terminal UI (ratatui) — history screen reads from runs.rs
│       ├── app.rs        # App state and Screen enum
│       ├── events.rs     # Keyboard event handlers
│       ├── mod.rs
│       ├── theme.rs      # Colors and styles
│       ├── ui.rs         # Render functions
│       └── widgets/      # UI widget components
├── error.rs              # Custom error types (AppError, AppResult)
├── util.rs               # Shared utilities (ps_quote, TempDirGuard, etc.)
├── workspace.rs          # Workspace layout helpers
├── runs.rs               # SQLite-backed run log (replaces the deleted history.rs)
├── search_index.rs       # SQLite-backed script search
├── lua_widget.rs         # Lua script widget execution
├── runtime.rs            # Script kind detection and command building
├── installer.rs          # Windows installer entrypoint (copies binary, updates PATH)
├── app_meta.rs           # App version constant
└── main.rs               # CLI routing and TUI entry
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
| `src/cli/run.rs` | Headless script execution; writes runs through `runs.rs` and emits a JSON envelope under `--json` |
| `src/cli/json.rs` | Single JSON envelope writer (`{ ok, data, error, schema_version }`) and stable error codes |
| `src/cli/describe.rs` | `omakure describe <script>` — full schema for one script |
| `src/cli/search.rs` | `omakure search <query>` — surfaces the SQLite script index |
| `src/cli/history.rs` | `omakure history list/show/tail` — queries `.history/runs.sqlite` |
| `src/cli/help_ai.rs` | `omakure help-ai` — single-call AI capability discovery generated from clap |
| `src/runs.rs` | SQLite run log: open, init_schema, insert_run, get_run, query_runs, generate_run_id |
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
    History,        // H key execution history
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
- **Constants**: SCREAMING_SNAKE_CASE (`BRAND_GRADIENT_START`, `ENV_HELP`)

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
│   └── <folder>/
│       └── index.lua         # Optional Lua widget
├── .history/
│   ├── search-index.sqlite   # Script search DB
│   └── *.json                # Execution logs
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

1. Create `src/cli/mycommand.rs`
2. Add `pub mod mycommand;` to `src/cli/mod.rs`
3. Add match arm in `main()` for the command string
4. Implement:
   - `pub struct MyCommandOptions { ... }`
   - `pub fn print_help() { ... }` - use `super::ENV_HELP` constant
   - `pub fn parse_args(...) -> Result<MyCommandOptions, Box<dyn Error>>`
   - `pub fn run(options: MyCommandOptions) -> Result<(), Box<dyn Error>>`

Example pattern from `src/cli/doctor.rs`:
```rust
use super::ENV_HELP;

pub fn print_help() {
    println!("Usage: omakure mycommand\n\n{ENV_HELP}");
}
```

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

Tests are located in:
- `src/domain/mod.rs` - Schema parsing, input normalization (12 tests)
- `src/runs.rs` - SQLite run log: schema init, insert/get/query, legacy json cleanup, run id generation, timestamp formatting
- `src/util.rs` - PowerShell quoting (3 tests)
- `src/error.rs` - Error type conversions (3 tests)

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

- `development.md` - Dev setup and architecture overview
- `how-to-create-a-script.md` - Script template and schema guide
- `environments.md` - Environment defaults system
- `lua-widgets.md` - Widget format and examples
- `workspace.md` - Workspace structure
- `usage.md` - CLI usage

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
