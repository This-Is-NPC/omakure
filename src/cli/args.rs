use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Omakure - TUI and CLI for navigating, running, and scheduling automation scripts.
///
/// Run `omakure` with no arguments to open the TUI against the global
/// workspace. Pass a path (e.g. `omakure .`) to open the TUI against any
/// directory without touching global history or environments.
///
/// Non-TUI surfaces:{n}
///   run <SCRIPT>          execute a script directly{n}
///   queue add <SCRIPT>    push a job; `queue worker` drains it{n}
///   serve                 run the cron scheduler daemon{n}
///   history list|show     query past runs (SQLite-backed){n}
///   scripts|describe|search   inspect the script catalogue
///
/// AI integration: pass `--json` on supported subcommands to emit a
/// `{ ok, data, error, schema_version }` envelope; run `omakure help-ai`
/// for the full machine-readable capability surface.
#[derive(Parser, Debug)]
#[command(name = "omakure")]
#[command(author, version, about)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Scripts directory override
    #[arg(long, global = true)]
    pub scripts_dir: Option<PathBuf>,

    /// Emit machine-readable JSON output for AI-facing subcommands.
    ///
    /// When set, supported subcommands print exactly one JSON envelope
    /// `{ ok, data, error, schema_version }` on stdout instead of their
    /// human-readable form. Subcommands that do not support JSON ignore
    /// this flag.
    #[arg(long, global = true)]
    pub json: bool,

    /// Open the TUI against the given directory as a session-only scripts
    /// root. History, environments, and workspace config stay anchored to
    /// the global workspace; only script listings, the root `index.lua`,
    /// and an optional `<PATH>/omakure.conf` session env are read from
    /// `<PATH>`. Mutually exclusive with `--scripts-dir`.
    #[arg(value_name = "PATH", conflicts_with = "scripts_dir")]
    pub path: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run a script without the TUI
    Run(RunArgs),

    /// Check runtime dependencies and workspace
    ///
    /// Verifies required interpreters (`git`, `bash`, `jq`), optional ones
    /// (`powershell`, `python`), workspace layout (`.omakure/`, history dir,
    /// workspace config), and that every script's embedded schema parses.
    /// Exits 1 if any required check fails. `--json` is currently ignored
    /// by this subcommand.
    #[command(visible_alias = "check")]
    Doctor,

    /// List available scripts
    Scripts(ScriptsArgs),

    /// Show the full schema of one script
    Describe(DescribeArgs),

    /// Search the script index
    Search(SearchArgs),

    /// Query the run history
    History(HistoryArgs),

    /// Push, cancel, drain, and inspect the run queue
    Queue(QueueArgs),

    /// Manage reusable Battery automation repositories
    Battery(BatteryArgs),

    /// Run the internal HTTP management API
    ///
    /// Starts a loopback-only HTTP API by default at `127.0.0.1:7878`.
    /// All endpoints except `/v1/health` require `Authorization: Bearer <token>`
    /// using `OMAKURE_API_TOKEN`. Binding to non-loopback addresses requires
    /// `--allow-non-loopback` and should only be used behind trusted network
    /// controls.
    Api(ApiArgs),

    /// Append a structured trace event from inside a running script
    Trace(TraceArgs),

    /// Print the AI capability surface as JSON
    ///
    /// Always emits JSON (regardless of `--json`). The envelope uses the
    /// standard `{ ok, data, error, schema_version }` shape.{n}
    /// {n}
    /// `data` contains:{n}
    ///   trust_model   — how omakure treats AI callers{n}
    ///   error_codes   — the registered stable error code strings{n}
    ///   envelope      — a self-describing shape hint{n}
    ///   verbs         — AI-relevant subcommands with flags and nested
    ///                   subcommands (pulled from clap metadata, so it
    ///                   cannot drift from `--help`){n}
    ///   data_shapes   — concrete examples for `run`, `history_list`,
    ///                   `history_show`, and `config`
    ///
    /// Agents can cache the payload per binary version (`--version`).
    HelpAi,

    /// Create a new script template
    Init(InitArgs),

    /// Show resolved paths and environment
    ///
    /// Prints the resolved binary path, omakure version, workspace root,
    /// scripts root, `.omakure/` directory, history directory, workspace
    /// config file, environments directory, active environment, and any
    /// known env overrides (`OMAKURE_SCRIPTS_DIR`, `OMAKURE_REPO`,
    /// `OVERTURE_*`, `CLOUD_MGMT_*`, `REPO`, `VERSION`). Pass `--json`
    /// for the machine-readable envelope.
    #[command(visible_alias = "env")]
    Config,

    /// Update omakure from GitHub releases
    ///
    /// Downloads the release archive for the current OS/arch and
    /// replaces the running binary in place. Also copies any scripts
    /// missing from your local scripts directory from the source
    /// archive of the target version — existing files are never
    /// overwritten. `--repo` defaults to `$OMAKURE_REPO` /
    /// `$OVERTURE_REPO` / `$CLOUD_MGMT_REPO` / `$REPO` /
    /// `This-Is-NPC/omakure`; `--version` defaults to `$VERSION` or the
    /// latest GitHub release.
    Update(UpdateArgs),

    /// Remove the omakure binary (optionally wipe the scripts workspace)
    ///
    /// Deletes the currently running binary from its install directory
    /// (on Windows also strips the install path from the user `PATH`).
    /// With `--scripts`, PERMANENTLY deletes the entire scripts
    /// workspace, including `.omakure/` (envs, daemon files), `.history/`,
    /// schedules) and every script file — use with care and have
    /// backups.
    Uninstall(UninstallArgs),

    /// Generate shell completion script for the given shell
    ///
    /// Writes the completion script to stdout. Quick install examples:{n}
    ///   bash: `omakure completion bash >> ~/.bashrc`{n}
    ///   zsh:  `omakure completion zsh  > ~/.zfunc/_omakure` (ensure `~/.zfunc` is on `$fpath`){n}
    ///   fish: `omakure completion fish > ~/.config/fish/completions/omakure.fish`{n}
    ///   pwsh: `omakure completion pwsh | Out-String | Invoke-Expression`
    ///
    /// For a one-shot session pipe into your current shell:
    /// `eval "$(omakure completion zsh)"`.
    Completion(CompletionArgs),

    /// Manage themes
    Theme(ThemeArgs),

    /// Run the cron scheduler daemon for scripts declaring a `Schedule` block
    ///
    /// Running `omakure serve` with no flags starts the scheduler in the
    /// foreground with an in-process worker; `-d`/`--detach` daemonizes
    /// (Unix) and `--stop` terminates a running daemon.
    ///
    /// The scheduler rescans the workspace every 5 seconds, parses each
    /// script's `Schedule` block, and enqueues a run when the cron
    /// expression is due. Fires are SKIPPED when a previous run with the
    /// same `cron_schedule_id` is still `queued` or `running`, so
    /// long-lived overlapping jobs never stack up.
    ///
    /// Paths (per workspace):{n}
    ///   PID file: `<workspace>/.omakure/daemon.pid`{n}
    ///   Log:      `<workspace>/.omakure/daemon.log`
    ///
    /// `--install`/`--uninstall`/`--status` manage a per-workspace
    /// systemd user unit so the daemon survives reboots (Linux only);
    /// after install tail with `journalctl --user -u <unit> -f`.
    ///
    /// By default an in-process worker is spawned so scheduled rows
    /// execute without a separate process. Pass `--no-worker` when you
    /// run `omakure queue worker` elsewhere.
    Serve(ServeArgs),
}

#[derive(Args, Debug)]
pub struct ScriptsArgs {
    /// Filter by tag (repeatable; AND semantics, case-sensitive literal
    /// match against the script's embedded `Tags` field).
    #[arg(long = "tag")]
    pub tag: Vec<String>,
}

#[derive(Args, Debug)]
pub struct DescribeArgs {
    /// Script name or path
    #[arg(value_name = "SCRIPT")]
    pub script: String,
}

#[derive(Args, Debug)]
pub struct ApiArgs {
    /// Address to bind the HTTP API server to
    #[arg(long, default_value = "127.0.0.1:7878")]
    pub bind: std::net::SocketAddr,

    /// Explicitly allow binding to non-loopback addresses
    #[arg(long)]
    pub allow_non_loopback: bool,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Free-text query (matches name, description, tags, fields)
    #[arg(value_name = "QUERY", default_value = "")]
    pub query: String,

    /// Filter by tag (repeatable; AND semantics, case-sensitive literal
    /// match against the script's embedded `Tags` field).
    #[arg(long = "tag")]
    pub tag: Vec<String>,
}

#[derive(Args, Debug)]
pub struct HistoryArgs {
    #[command(subcommand)]
    pub command: HistoryCommand,
}

#[derive(Subcommand, Debug)]
pub enum HistoryCommand {
    /// List recent runs
    List(HistoryListArgs),

    /// Show one run by id
    Show(HistoryShowArgs),

    /// Print the most recent N runs (no --follow in v1)
    Tail(HistoryTailArgs),

    /// Aggregate counts per state and per actor
    Stats,

    /// Read the structured trace stream of one run
    Traces(HistoryTracesArgs),
}

#[derive(Args, Debug)]
pub struct HistoryListArgs {
    /// Filter by script name or path substring
    #[arg(long)]
    pub script: Option<String>,

    /// Filter by actor tag (e.g. `human`, `ai`)
    #[arg(long)]
    pub actor: Option<String>,

    /// Only runs since this duration ago (e.g. `1d`, `30m`, `12h`)
    #[arg(long)]
    pub since: Option<String>,

    /// Only runs until this duration ago
    #[arg(long)]
    pub until: Option<String>,

    /// Only successful runs
    #[arg(long, conflicts_with = "failure")]
    pub success: bool,

    /// Only failed runs
    #[arg(long, conflicts_with = "success")]
    pub failure: bool,

    /// Maximum number of rows to return
    #[arg(long)]
    pub limit: Option<i64>,

    /// Filter by run state (repeatable; logical OR within the flag).
    /// Valid values: queued, running, completed, failed, cancelled,
    /// timed_out, dead_letter. Mutually exclusive with `--state-set`.
    #[arg(long = "state", conflicts_with = "state_set")]
    pub state: Vec<String>,

    /// Filter by a named state group: `in_flight` (queued+running),
    /// `terminal` (everything else), or `all`. Default when neither
    /// `--state` nor `--state-set` is set: `terminal` so existing
    /// callers see no behavior change.
    #[arg(long = "state-set", conflicts_with = "state")]
    pub state_set: Option<String>,
}

#[derive(Args, Debug)]
pub struct HistoryTracesArgs {
    /// Run id
    #[arg(value_name = "RUN_ID")]
    pub run_id: String,

    /// Minimum level (debug, info, warn, error). Defaults to `debug`
    /// (returns every record).
    #[arg(long)]
    pub level: Option<String>,

    /// Return only entries with `sequence > N`. Used by agents for
    /// incremental fetches.
    #[arg(long = "since-sequence")]
    pub since_sequence: Option<i64>,
}

#[derive(Args, Debug)]
pub struct HistoryShowArgs {
    /// Run id (as printed by `omakure run --json` or `omakure history list`)
    #[arg(value_name = "RUN_ID")]
    pub run_id: String,
}

#[derive(Args, Debug)]
pub struct HistoryTailArgs {
    /// Number of rows to print (default: 10)
    #[arg(long, default_value_t = 10)]
    pub limit: i64,

    /// Reserved for future use; rejected with error.code = "not_implemented"
    #[arg(long)]
    pub follow: bool,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Script name or path
    #[arg(value_name = "SCRIPT")]
    pub script: String,

    /// Actor tag recorded in the run history (default: `human`).
    #[arg(long, default_value = "human")]
    pub actor: String,

    /// Optional free-form reason recorded in the run history.
    #[arg(long)]
    pub reason: Option<String>,

    /// Caller-provided run id; otherwise a fresh id is generated.
    #[arg(long = "run-id")]
    pub run_id: Option<String>,

    /// Optional parent run id, for chained agent workflows.
    #[arg(long = "parent-run-id")]
    pub parent_run_id: Option<String>,

    /// Fail with a structured error if any required field is missing
    /// instead of attempting to read stdin / open a TTY. Implied by
    /// `--json`.
    #[arg(long = "no-prompt")]
    pub no_prompt: bool,

    /// Path to an env file whose `KEY=value` pairs are injected into the
    /// script process for this run only. Values override the managed
    /// active env for the same key, but omakure-reserved vars
    /// (`OMAKURE_RUN_ID`, `OMAKURE_SCRIPTS_DIR`) always win. A missing or
    /// unreadable path is a hard error.
    ///
    /// Example: `omakure run deploy --env-file ./.venv.env -- --target prod`
    #[arg(long = "env-file", value_name = "PATH")]
    pub env_file: Option<PathBuf>,

    /// Arguments forwarded to the script
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Script path
    #[arg(value_name = "SCRIPT", required_unless_present = "name")]
    pub script: Option<String>,

    /// Script path (legacy)
    #[arg(long, value_name = "SCRIPT")]
    pub name: Option<String>,

    /// Inline schema JSON or `@path/to/schema.json`. When set, the new
    /// script is generated with this schema embedded between the
    /// `OMAKURE_SCHEMA_START` / `OMAKURE_SCHEMA_END` markers instead of
    /// the default placeholder template.
    #[arg(long = "schema-json")]
    pub schema_json: Option<String>,

    /// Read the script body from stdin and write it verbatim under the
    /// schema header. Useful when an agent ships both schema and body in
    /// one call.
    #[arg(long = "body-stdin")]
    pub body_stdin: bool,

    /// Overwrite an existing script of the same name.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
#[command(disable_version_flag = true)]
pub struct UpdateArgs {
    /// GitHub repository (`owner/name`). Defaults to `$OMAKURE_REPO` /
    /// `$OVERTURE_REPO` / `$CLOUD_MGMT_REPO` / `$REPO` /
    /// `This-Is-NPC/omakure`.
    #[arg(long)]
    pub repo: Option<String>,

    /// Release tag to install (e.g. `v0.1.9`). Defaults to `$VERSION`
    /// or the latest GitHub release for the configured repo.
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Also delete the scripts workspace directory (runs.sqlite,
    /// history, schedules, and every user script). Destructive.
    #[arg(long)]
    pub scripts: bool,
}

#[derive(Args, Debug)]
pub struct CompletionArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: Shell,
}

#[derive(Args, Debug)]
pub struct ThemeArgs {
    #[command(subcommand)]
    pub command: ThemeCommand,
}

#[derive(Subcommand, Debug)]
pub enum ThemeCommand {
    /// List available themes
    List,

    /// Set the default theme
    Set(ThemeSetArgs),

    /// Preview a theme
    Preview(ThemeSetArgs),

    /// Print theme paths
    Path,
}

#[derive(Args, Debug)]
pub struct ThemeSetArgs {
    /// Theme name
    #[arg(value_name = "NAME")]
    pub name: String,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Pwsh,
}

// ---------------------------------------------------------------------------
// Queue subcommand
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct QueueArgs {
    #[command(subcommand)]
    pub command: QueueCommand,
}

#[derive(Args, Debug)]
pub struct BatteryArgs {
    #[command(subcommand)]
    pub command: BatteryCommand,
}

#[derive(Subcommand, Debug)]
pub enum BatteryCommand {
    /// List registered Batteries
    List,

    /// Register a Battery repository source
    Add(BatteryAddArgs),

    /// Fetch and validate a Battery checkout
    Sync(BatteryNameArgs),

    /// Inspect one synced Battery manifest
    Inspect(BatteryNameArgs),

    /// List installable scripts from one Battery
    Scripts(BatteryNameArgs),

    /// Install one Battery script into the trusted scripts workspace
    Install(BatteryInstallArgs),

    /// Unregister one Battery
    Remove(BatteryRemoveArgs),
}

#[derive(Args, Debug)]
pub struct BatteryAddArgs {
    /// Git repository URL or local path
    #[arg(value_name = "GIT_URL")]
    pub git_url: String,

    /// Stable Battery name (lowercase kebab-case)
    #[arg(long)]
    pub name: String,

    /// Branch, tag, or ref to sync
    #[arg(long = "ref", default_value = "main")]
    pub requested_ref: String,
}

#[derive(Args, Debug)]
pub struct BatteryNameArgs {
    /// Battery name
    #[arg(value_name = "NAME")]
    pub name: String,
}

#[derive(Args, Debug)]
pub struct BatteryInstallArgs {
    /// Battery name
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Script id from `omakure battery scripts <name>`
    #[arg(value_name = "SCRIPT_ID")]
    pub script_id: String,

    /// Overwrite an existing script target
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct BatteryRemoveArgs {
    /// Battery name
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Also delete the cached clone
    #[arg(long = "remove-cache")]
    pub remove_cache: bool,
}

#[derive(Subcommand, Debug)]
pub enum QueueCommand {
    /// Push a job onto the queue
    Add(QueueAddArgs),

    /// Cancel a queued or running job
    Cancel(QueueCancelArgs),

    /// Promote a `failed` or `timed_out` row into `dead_letter`
    DeadLetter(QueueDeadLetterArgs),

    /// Drain the queue (long-running daemon)
    Worker(QueueWorkerArgs),

    /// Aggregate counts per state and per actor
    Stats,
}

#[derive(Args, Debug)]
pub struct QueueAddArgs {
    /// Script name or path
    #[arg(value_name = "SCRIPT")]
    pub script: String,

    /// Actor tag recorded on the row (default: `human`)
    #[arg(long, default_value = "human")]
    pub actor: String,

    /// Optional free-form reason
    #[arg(long)]
    pub reason: Option<String>,

    /// Higher value picked first (default 0)
    #[arg(long, default_value_t = 0)]
    pub priority: i64,

    /// Per-job execution timeout (e.g. `30s`, `5m`, `1h`).
    /// Without this flag the job has no execution limit.
    #[arg(long)]
    pub timeout: Option<String>,

    /// Optional parent run id, for chained agent workflows
    #[arg(long = "parent-run-id")]
    pub parent_run_id: Option<String>,

    /// Caller-provided run id; otherwise a fresh id is generated
    #[arg(long = "run-id")]
    pub run_id: Option<String>,

    /// Provenance id tying this row to a named cron schedule. Populated
    /// automatically by `omakure serve`; set manually only to replay or
    /// simulate a scheduled run.
    #[arg(long = "cron-schedule-id")]
    pub cron_schedule_id: Option<String>,

    /// Arguments forwarded to the script (after `--`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("omakure").chain(args.iter().copied()))
    }

    #[test]
    fn test_parse_no_args_opens_tui() {
        let cli = parse(&[]).unwrap();
        assert!(cli.command.is_none());
        assert!(cli.path.is_none());
    }

    #[test]
    fn test_parse_positional_path() {
        let cli = parse(&["/some/path"]).unwrap();
        assert_eq!(cli.path, Some(PathBuf::from("/some/path")));
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_parse_run_subcommand() {
        let cli = parse(&["run", "deploy.sh"]).unwrap();
        match cli.command.unwrap() {
            Commands::Run(args) => {
                assert_eq!(args.script, "deploy.sh");
                assert_eq!(args.actor, "human");
                assert!(!args.no_prompt);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn test_parse_run_with_flags() {
        let cli = parse(&["run", "deploy.sh", "--json", "--no-prompt", "--actor", "ai"]).unwrap();
        assert!(cli.json);
        match cli.command.unwrap() {
            Commands::Run(args) => {
                assert_eq!(args.actor, "ai");
                assert!(args.no_prompt);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn test_parse_global_json_flag() {
        let cli = parse(&["--json", "scripts"]).unwrap();
        assert!(cli.json);
    }

    #[test]
    fn test_omaken_list_command_is_removed() {
        let cli = parse(&["list"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.path, Some(PathBuf::from("list")));
    }

    #[test]
    fn test_omaken_install_command_is_removed() {
        let result = parse(&["install", "https://example.com/scripts.git"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_queue_add() {
        let cli = parse(&["queue", "add", "deploy.sh", "--priority", "10"]).unwrap();
        match cli.command.unwrap() {
            Commands::Queue(q) => match q.command {
                QueueCommand::Add(args) => {
                    assert_eq!(args.script, "deploy.sh");
                    assert_eq!(args.priority, 10);
                }
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Queue"),
        }
    }

    #[test]
    fn test_parse_battery_add() {
        let cli = parse(&[
            "battery",
            "add",
            "https://example.invalid/azure.git",
            "--name",
            "azure",
            "--ref",
            "stable",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Commands::Battery(args) => match args.command {
                BatteryCommand::Add(add) => {
                    assert_eq!(add.git_url, "https://example.invalid/azure.git");
                    assert_eq!(add.name, "azure");
                    assert_eq!(add.requested_ref, "stable");
                }
                _ => panic!("expected Battery Add"),
            },
            _ => panic!("expected Battery"),
        }
    }

    #[test]
    fn test_parse_battery_install_force() {
        let cli = parse(&["battery", "install", "azure", "azure.list", "--force"]).unwrap();
        match cli.command.unwrap() {
            Commands::Battery(args) => match args.command {
                BatteryCommand::Install(install) => {
                    assert_eq!(install.name, "azure");
                    assert_eq!(install.script_id, "azure.list");
                    assert!(install.force);
                }
                _ => panic!("expected Battery Install"),
            },
            _ => panic!("expected Battery"),
        }
    }

    #[test]
    fn test_parse_battery_remove_cache() {
        let cli = parse(&["battery", "remove", "azure", "--remove-cache"]).unwrap();
        match cli.command.unwrap() {
            Commands::Battery(args) => match args.command {
                BatteryCommand::Remove(remove) => {
                    assert_eq!(remove.name, "azure");
                    assert!(remove.remove_cache);
                }
                _ => panic!("expected Battery Remove"),
            },
            _ => panic!("expected Battery"),
        }
    }

    #[test]
    fn test_parse_api_defaults() {
        let cli = parse(&["api"]).unwrap();
        match cli.command.unwrap() {
            Commands::Api(args) => {
                assert_eq!(args.bind.to_string(), "127.0.0.1:7878");
                assert!(!args.allow_non_loopback);
            }
            _ => panic!("expected Api"),
        }
    }

    #[test]
    fn test_parse_api_bind_flags() {
        let cli = parse(&["api", "--bind", "0.0.0.0:7878", "--allow-non-loopback"]).unwrap();
        match cli.command.unwrap() {
            Commands::Api(args) => {
                assert_eq!(args.bind.to_string(), "0.0.0.0:7878");
                assert!(args.allow_non_loopback);
            }
            _ => panic!("expected Api"),
        }
    }

    #[test]
    fn test_api_help_surface_exists() {
        let command = Cli::command();
        let api = command
            .find_subcommand("api")
            .expect("api subcommand should be registered");
        assert!(api.get_arguments().any(|arg| arg.get_id() == "bind"));
        assert!(api
            .get_arguments()
            .any(|arg| arg.get_id() == "allow_non_loopback"));
    }

    #[test]
    fn test_parse_queue_worker() {
        let cli = parse(&["queue", "worker", "--concurrency", "4"]).unwrap();
        match cli.command.unwrap() {
            Commands::Queue(q) => match q.command {
                QueueCommand::Worker(args) => assert_eq!(args.concurrency, 4),
                _ => panic!("expected Worker"),
            },
            _ => panic!("expected Queue"),
        }
    }

    #[test]
    fn test_parse_history_list_with_state() {
        let cli = parse(&["history", "list", "--state", "completed", "--since", "1h"]).unwrap();
        match cli.command.unwrap() {
            Commands::History(h) => match h.command {
                HistoryCommand::List(args) => {
                    assert_eq!(args.state, vec!["completed"]);
                    assert_eq!(args.since, Some("1h".to_string()));
                }
                _ => panic!("expected List"),
            },
            _ => panic!("expected History"),
        }
    }

    #[test]
    fn test_parse_history_show() {
        let cli = parse(&["history", "show", "abc123"]).unwrap();
        match cli.command.unwrap() {
            Commands::History(h) => match h.command {
                HistoryCommand::Show(args) => assert_eq!(args.run_id, "abc123"),
                _ => panic!("expected Show"),
            },
            _ => panic!("expected History"),
        }
    }

    #[test]
    fn test_conflicting_scripts_dir_and_path() {
        let result = parse(&["--scripts-dir", "/a", "/b"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_describe() {
        let cli = parse(&["describe", "deploy.sh"]).unwrap();
        match cli.command.unwrap() {
            Commands::Describe(args) => assert_eq!(args.script, "deploy.sh"),
            _ => panic!("expected Describe"),
        }
    }

    #[test]
    fn test_parse_search() {
        let cli = parse(&["search", "deploy", "--tag", "infra"]).unwrap();
        match cli.command.unwrap() {
            Commands::Search(args) => {
                assert_eq!(args.query, "deploy");
                assert_eq!(args.tag, vec!["infra"]);
            }
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn test_parse_init_with_schema() {
        let cli = parse(&["init", "new.sh", "--schema-json", "{}"]).unwrap();
        match cli.command.unwrap() {
            Commands::Init(args) => {
                assert_eq!(args.script, Some("new.sh".to_string()));
                assert_eq!(args.schema_json, Some("{}".to_string()));
            }
            _ => panic!("expected Init"),
        }
    }

    #[test]
    fn test_parse_theme_set() {
        let cli = parse(&["theme", "set", "dracula"]).unwrap();
        match cli.command.unwrap() {
            Commands::Theme(t) => match t.command {
                ThemeCommand::Set(args) => assert_eq!(args.name, "dracula"),
                _ => panic!("expected Set"),
            },
            _ => panic!("expected Theme"),
        }
    }

    #[test]
    fn test_parse_trace() {
        let cli = parse(&["trace", "hello", "--level", "warn", "--data", "{\"k\":1}"]).unwrap();
        match cli.command.unwrap() {
            Commands::Trace(args) => {
                assert_eq!(args.message, "hello");
                assert_eq!(args.level, "warn");
                assert_eq!(args.data, Some("{\"k\":1}".to_string()));
            }
            _ => panic!("expected Trace"),
        }
    }

    #[test]
    fn test_history_success_failure_conflict() {
        let result = parse(&["history", "list", "--success", "--failure"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_history_state_and_state_set_conflict() {
        let result = parse(&[
            "history",
            "list",
            "--state",
            "completed",
            "--state-set",
            "all",
        ]);
        assert!(result.is_err());
    }
}

#[derive(Args, Debug)]
pub struct QueueCancelArgs {
    /// Run id to cancel
    #[arg(value_name = "RUN_ID")]
    pub run_id: String,

    /// Optional reason recorded on the cancelled row
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Args, Debug)]
pub struct QueueDeadLetterArgs {
    /// Run id to promote
    #[arg(value_name = "RUN_ID")]
    pub run_id: String,

    /// Optional reason appended to the row
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Args, Debug)]
pub struct QueueWorkerArgs {
    /// Number of parallel workers (default 1)
    #[arg(long, default_value_t = 1)]
    pub concurrency: u32,

    /// Only claim jobs whose actor matches this tag
    #[arg(long = "actor-filter")]
    pub actor_filter: Option<String>,

    /// Only claim jobs whose script path or name contains this pattern
    #[arg(long = "script-filter")]
    pub script_filter: Option<String>,

    /// Test convenience: drain at most one job per worker thread, then
    /// exit. Hidden from --help and help-ai. Used by integration tests
    /// so the daemon does not block the test harness.
    #[arg(long, hide = true)]
    pub once: bool,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Run the scheduler as a detached background daemon (Unix only).
    #[arg(long, short = 'd', conflicts_with_all = ["stop", "install", "uninstall", "status"])]
    pub detach: bool,

    /// Stop a running daemon (reads `.omakure/daemon.pid` and sends SIGTERM).
    #[arg(long, conflicts_with_all = ["install", "uninstall", "status"])]
    pub stop: bool,

    /// Install a systemd user service that runs `omakure serve` for the
    /// current workspace and survives reboots (Linux only).
    #[arg(long, conflicts_with_all = ["uninstall", "status"])]
    pub install: bool,

    /// Disable and remove the systemd user service for the current
    /// workspace (Linux only).
    #[arg(long, conflicts_with_all = ["status"])]
    pub uninstall: bool,

    /// Print the systemd user service status for the current workspace
    /// (Linux only).
    #[arg(long)]
    pub status: bool,

    /// Do not spawn the in-process worker. Use when you already run
    /// `omakure queue worker` elsewhere.
    #[arg(long = "no-worker")]
    pub no_worker: bool,

    /// Number of worker threads for the in-process worker (default 1).
    #[arg(long, default_value_t = 1)]
    pub concurrency: u32,

    /// Test convenience: run a single scheduler tick, enqueue whatever is
    /// due, then exit. Hidden from --help. Used by integration tests.
    #[arg(long, hide = true)]
    pub once: bool,
}

#[derive(Args, Debug)]
pub struct TraceArgs {
    /// Trace message
    #[arg(value_name = "MESSAGE")]
    pub message: String,

    /// Level (debug, info, warn, error). Defaults to `info`.
    #[arg(long, default_value = "info")]
    pub level: String,

    /// Optional structured payload (must parse as JSON)
    #[arg(long)]
    pub data: Option<String>,
}
