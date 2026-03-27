# Native Scheduling/Cron Support

## User Story

**As a** power user and automation enthusiast,
**I want to** define execution schedules (cron expressions) directly within the script's `omakure` JSON schema,
**So that** my scripts become fully self-contained "automation units" (Logic + UI + Schedule) that run automatically without needing external configuration (systemd/crontab) while keeping a unified execution history.

---

## Context & Motivation

Currently, `omakure` is excellent for manually triggered actions. However, for recurring tasks (backups, syncs, reports), users must rely on external tools like `cron` or `systemd`. This separates the "when" (schedule) from the "what" and "how" (script and parameters), breaking the portability of the scripts.

If `omakure` could handle scheduling natively via the script's schema, it would become the ultimate portable automation tool for local environments.

---

## Proposed Solution Overview

1. **Schema Extension:** Add a `schedule` field to the JSON schema block
2. **Daemon Mode:** New `omakure serve` command to run scheduler in background
3. **TUI Screens:** New screens to view and manage scheduled scripts
4. **Unified History:** Scheduled runs appear in the same history log, tagged as `SCHEDULED`

---

## Example Schema

```bash
#!/bin/bash
# OMAKURE_SCHEMA_START
# {
#   "Name": "Daily Backup",
#   "Description": "Backups critical files",
#   "Schedule": {
#     "Cron": "0 3 * * *",
#     "Enabled": true
#   },
#   "Fields": [
#     {
#       "Name": "target",
#       "Type": "string",
#       "Default": "/mnt/backup"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END
```

### Supported Cron Formats

| Format | Example | Description |
|--------|---------|-------------|
| Standard 5 fields | `0 3 * * *` | minute hour day month weekday |
| With seconds (6 fields) | `0 0 3 * * *` | second minute hour day month weekday |
| `@hourly` | - | Every hour at :00 |
| `@daily` / `@midnight` | - | Every day at 00:00 |
| `@weekly` | - | Every Sunday at 00:00 |
| `@monthly` | - | First day of month at 00:00 |
| `@yearly` / `@annually` | - | January 1st at 00:00 |

---

## Technical Implementation Plan

### Phase 1: Schema Extension
**Estimated effort:** 2-3 hours

#### 1.1 Update Schema Struct (`src/domain/schema.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub fields: Vec<Field>,
    pub outputs: Option<Vec<OutputField>>,
    pub queue: Option<QueueSpec>,
    pub schedule: Option<ScheduleConfig>,  // NEW
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    pub cron: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool { true }
```

#### 1.2 Add Cron Validation (`src/domain/validation.rs`)
- Validate cron expression on schema parse
- Return clear error message for invalid expressions
- Add dependency: `croner = "2"` to Cargo.toml

#### Tasks:
- [ ] Add `ScheduleConfig` struct to `schema.rs`
- [ ] Add `schedule` field to `Schema` struct
- [ ] Add cron validation in parsing pipeline
- [ ] Add `croner` dependency
- [ ] Write unit tests for cron parsing

---

### Phase 2: History Extension
**Estimated effort:** 1-2 hours

#### 2.1 Update HistoryEntry (`src/history.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: i64,
    pub script: PathBuf,
    pub args: Vec<String>,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    #[serde(default)]                     // NEW - backward compatible
    pub trigger: ExecutionTrigger,        // NEW
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum ExecutionTrigger {
    #[default]
    Manual,
    Scheduled,
}
```

#### Tasks:
- [ ] Add `ExecutionTrigger` enum
- [ ] Add `trigger` field with `#[serde(default)]` for backward compatibility
- [ ] Update `success_entry()` and `error_entry()` to accept trigger parameter
- [ ] Write tests to ensure old history files still load correctly

---

### Phase 3: CLI Serve Command
**Estimated effort:** 4-6 hours

#### 3.1 Add CLI Arguments (`src/cli/args.rs`)
```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    // ... existing commands ...
    
    /// Run the scheduler daemon
    Serve(ServeArgs),
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Run in background (daemon mode)
    #[arg(short, long)]
    pub detach: bool,
    
    /// Stop a running daemon
    #[arg(long)]
    pub stop: bool,
    
    /// Show daemon status
    #[arg(long)]
    pub status: bool,
}
```

#### 3.2 Create Scheduler Module (`src/scheduler/`)
```
src/scheduler/
├── mod.rs           # Module exports
├── cron_parser.rs   # Cron expression handling
├── executor.rs      # Script execution logic
└── daemon.rs        # Daemon/detach logic
```

#### 3.3 Implement Serve Command (`src/cli/serve.rs`)

**Foreground mode flow:**
```
1. Check for existing lock file (.omaken/daemon.lock)
2. If lock exists, check if PID is alive
   - If alive: error "Daemon already running"
   - If dead: remove stale lock
3. Create lock file with current PID
4. Main loop:
   a. Scan all scripts for schedule field
   b. For each scheduled script:
      - Parse cron expression
      - Check if current minute matches
      - If match: execute with default field values
      - Record to history with trigger=Scheduled
   c. Sleep until next minute boundary (:00)
5. On SIGINT/SIGTERM: remove lock file and exit
```

**Daemon mode (-d) flow:**
```
1. Fork process
2. Parent: print "Daemon started with PID X" and exit
3. Child:
   - Redirect stdout/stderr to .omaken/daemon.log
   - Write PID to .omaken/daemon.pid
   - Continue with foreground logic
```

**Stop command (--stop) flow:**
```
1. Read PID from .omaken/daemon.pid
2. Send SIGTERM to process
3. Wait for process to exit (with timeout)
4. Remove PID file
```

#### Tasks:
- [ ] Add `Serve` command to CLI args
- [ ] Create `src/scheduler/` module structure
- [ ] Implement cron matching logic
- [ ] Implement script execution with defaults
- [ ] Implement lock file management
- [ ] Implement daemon forking (Unix) or background process (Windows)
- [ ] Implement `--stop` command
- [ ] Implement `--status` command
- [ ] Add signal handlers for graceful shutdown
- [ ] Write integration tests

---

### Phase 4: TUI - Schedules Screen
**Estimated effort:** 4-5 hours

#### 4.1 Add Screen State (`src/adapters/tui/state/schedule_state.rs`)
```rust
pub struct ScheduleState {
    pub scripts: Vec<ScheduledScript>,
    pub selected_index: usize,
    pub scroll_offset: usize,
}

pub struct ScheduledScript {
    pub path: PathBuf,
    pub name: String,
    pub cron: String,
    pub enabled: bool,
    pub next_run: Option<DateTime<Local>>,
}
```

#### 4.2 Add Screen Enum Variant
```rust
pub enum Screen {
    // ... existing variants ...
    Schedules,        // NEW
    ScheduleDetail,   // NEW
}
```

#### 4.3 Create Schedules Widget (`src/adapters/tui/widgets/schedules.rs`)

**Layout:**
```
Scheduled Scripts

[Up/Down] Navigate  [Enter] Details  [e] Enable/Disable  [q] Back

Script               Cron           Next Run       Status
----------------------------------------------------------
> Daily Backup       0 3 * * *      2024-01-16 03:00   On
  Weekly Report      0 9 * * 1      2024-01-22 09:00   On
  Hourly Sync        @hourly        2024-01-15 15:00   Off
```

#### 4.4 Add Keyboard Handler
- `Up/Down` or `j/k`: Navigate list
- `Enter`: Open detail view
- `e`: Toggle enabled/disabled
- `q` or `Esc`: Back to main screen
- `r`: Refresh list

#### Tasks:
- [ ] Create `ScheduleState` struct
- [ ] Add `Screen::Schedules` variant
- [ ] Create schedules list widget
- [ ] Add keybinding `S` on main screen to open Schedules
- [ ] Implement navigation and selection
- [ ] Implement enable/disable toggle (writes to script file)

---

### Phase 5: TUI - Schedule Detail Screen
**Estimated effort:** 3-4 hours

#### 5.1 Create Detail Widget

**Layout:**
```
Daily Backup

Schedule: 0 3 * * *
Next Run: 2024-01-16 03:00
Status:   Enabled

Parameters (defaults):
----------------------
--target: /mnt/backup
--compress: true

History (filtered)
2024-01-15 03:00  OK   [AUTO]
2024-01-14 03:00  OK   [AUTO]
2024-01-13 03:00  FAIL [AUTO]

[r] Run Now  [e] Edit  [d] Disable  [Enter] View Output
```

#### 5.2 Add Keyboard Handler
- `Left/Right` or `h/l`: Switch focus between panels
- `Up/Down` or `j/k`: Navigate history (when focused)
- `Enter`: View full output of selected history entry
- `r`: Run script now (manual trigger)
- `e`: Toggle enabled
- `q` or `Esc`: Back to schedules list

#### Tasks:
- [ ] Create detail widget with split layout
- [ ] Implement history filtering by script path
- [ ] Add visual indicator for scheduled vs manual runs
- [ ] Implement "Run Now" action
- [ ] Add navigation between panels

---

### Phase 6: Visual Indicators
**Estimated effort:** 1-2 hours

#### 6.1 Script List Indicator
In `src/adapters/tui/widgets/scripts.rs`:
- Add `[S]` prefix for scripts with schedule

#### 6.2 History List Indicator
In `src/adapters/tui/widgets/history.rs`:
- Add `[AUTO]` suffix or different color for scheduled executions

#### Tasks:
- [ ] Add schedule indicator to script list
- [ ] Add trigger indicator to history list
- [ ] Update theme to include schedule-related colors

---

## File Structure Summary

### New Files
```
src/
├── cli/
│   └── serve.rs              # Serve command implementation
├── scheduler/
│   ├── mod.rs                # Module exports
│   ├── cron_parser.rs        # Cron expression handling
│   ├── executor.rs           # Scheduled execution logic
│   └── daemon.rs             # Daemon management
├── adapters/tui/
│   ├── state/
│   │   └── schedule_state.rs # Schedule screens state
│   └── widgets/
│       ├── schedules.rs      # Schedules list widget
│       └── schedule_detail.rs # Detail view widget
```

### Modified Files
```
src/
├── domain/
│   ├── schema.rs             # Add ScheduleConfig
│   └── validation.rs         # Add cron validation
├── history.rs                # Add ExecutionTrigger
├── cli/
│   ├── args.rs               # Add Serve command
│   └── mod.rs                # Register serve module
├── adapters/tui/
│   ├── app.rs                # Add schedule-related state
│   ├── events.rs             # Add keyboard handlers
│   ├── ui.rs                 # Add screen rendering
│   └── widgets/
│       ├── scripts.rs        # Add schedule indicator
│       └── history.rs        # Add trigger indicator
├── main.rs                   # Handle serve command
└── Cargo.toml                # Add croner dependency
```

---

## Dependencies

```toml
# Add to Cargo.toml
[dependencies]
croner = "2"           # Cron expression parsing
# Note: daemonize crate is optional, can use std::process::Command for simple fork
```

---

## Acceptance Criteria

### Schema
- [ ] `Schedule` field accepted in schema parser
- [ ] Supports 5-field cron expressions (minute hour day month weekday)
- [ ] Supports 6-field cron expressions (with seconds)
- [ ] Supports cron macros (@hourly, @daily, @weekly, @monthly, @yearly)
- [ ] Clear error message for invalid cron expressions
- [ ] `Enabled` field defaults to `true`

### CLI - Serve Command
- [ ] `omakure serve` runs scheduler in foreground
- [ ] `omakure serve -d` runs scheduler as daemon
- [ ] `omakure serve --stop` stops running daemon
- [ ] `omakure serve --status` shows daemon status
- [ ] Lock file prevents multiple daemon instances
- [ ] Graceful shutdown on SIGINT/SIGTERM
- [ ] Logs written to `.omaken/daemon.log` in daemon mode

### Scheduler Behavior
- [ ] Scripts executed at correct times based on cron expression
- [ ] Scripts use `Default` values from field definitions
- [ ] Execution errors logged but do not stop scheduler
- [ ] Disabled schedules are skipped

### History
- [ ] New `trigger` field (Manual/Scheduled)
- [ ] Backward compatible with existing history files
- [ ] Scheduled executions clearly marked in history

### TUI - Schedules Screen
- [ ] Accessible via `S` key from main screen
- [ ] Lists all scripts with schedule defined
- [ ] Shows cron expression and next run time
- [ ] Shows enabled/disabled status
- [ ] Navigate with Up/Down or j/k
- [ ] Toggle enabled with `e` key
- [ ] Open detail with Enter

### TUI - Schedule Detail Screen
- [ ] Shows schedule configuration
- [ ] Shows parameter defaults that will be used
- [ ] Shows filtered history (only this script)
- [ ] Distinguishes manual vs scheduled runs visually
- [ ] "Run Now" action available with `r` key

### Visual Indicators
- [ ] Scripts with schedule show indicator in main list
- [ ] History entries show trigger type (manual/scheduled)

---

## Estimated Total Effort

| Phase | Description | Estimate |
|-------|-------------|----------|
| 1 | Schema Extension | 2-3 hours |
| 2 | History Extension | 1-2 hours |
| 3 | CLI Serve Command | 4-6 hours |
| 4 | TUI Schedules Screen | 4-5 hours |
| 5 | TUI Schedule Detail | 3-4 hours |
| 6 | Visual Indicators | 1-2 hours |
| **Total** | | **15-22 hours** |

---

## Future Enhancements (Out of Scope)

- [ ] Web UI for schedule management
- [ ] Email/webhook notifications on failure
- [ ] Schedule dependencies (run B after A completes)
- [ ] Timezone support in cron expressions
- [ ] Concurrency limits (max N scheduled scripts running simultaneously)
- [ ] Retry logic for failed scheduled executions
