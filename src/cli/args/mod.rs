use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod api;
mod battery;
mod env;
mod history;
mod node;
mod queue;
mod scripts;
mod serve;
mod setup;

pub use api::{ApiArgs, TokenArgs, TokenCommand, TokenGenerateArgs};
pub use battery::{
    BatteryAddArgs, BatteryArgs, BatteryCommand, BatteryInstallArgs, BatteryNameArgs,
    BatteryRemoveArgs,
};
pub use env::{EnvArgs, EnvCommand, EnvCreateArgs, EnvNameArgs, EnvRemoveArgs, EnvSetArgs};
pub use history::{
    HistoryArgs, HistoryCommand, HistoryListArgs, HistoryShowArgs, HistoryTailArgs,
    HistoryTracesArgs, TraceArgs,
};
pub use node::{
    NodeArgs, NodeAuthorityArgs, NodeAuthorityCommand, NodeAuthorityCreateArgs,
    NodeAuthorityIssueArgs, NodeBaselineArgs, NodeBaselineCommand, NodeBaselinePublishArgs,
    NodeBaselinePushArgs, NodeBaselineRollbackArgs, NodeCapabilitiesArgs, NodeCommand, NodeCueArgs,
    NodeDirectProbeArgs, NodeDiscoveryArgs, NodeEnrollApplyArgs, NodeEnrollApproveArgs,
    NodeEnrollArgs, NodeEnrollCommand, NodeEnrollRejectArgs, NodeEnrollRequestArgs, NodeResetArgs,
    NodeRevokeArgs, NodeServeArgs, NodeTrustArgs,
};
pub use queue::{
    QueueAddArgs, QueueArgs, QueueCancelArgs, QueueCommand, QueueDeadLetterArgs, QueueWorkerArgs,
};
pub use scripts::{DescribeArgs, RunArgs, ScriptsArgs, SearchArgs};
pub use serve::ServeArgs;
pub use setup::{CompletionArgs, InitArgs, Shell, UninstallArgs, UpdateArgs};

/// Omakure - CLI for running and scheduling automation scripts.
///
/// Run `omakure` with no arguments to print this help.
///
/// CLI surfaces:{n}
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

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run a script directly
    ///
    /// Without `--no-prompt` or `--json`, missing required schema fields
    /// prompt on stdin or a TTY. Scripts generated with `omakure init` may
    /// also prompt for values such as an optional `target`. When stdin is not
    /// a TTY (closed stdin, pipes), those prompts can fail with exit code 1
    /// and no extra omakure diagnostic — pass arguments after `--` or use
    /// `--no-prompt`.
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

    /// Generate hashed API tokens for `--tokens-file` auth
    ///
    /// Prints a plaintext token once (prefix `omk_live_`), its Argon2id PHC
    /// hash, and a TOML `[[tokens]]` entry. Does not append to a secrets file
    /// unless `--append` is passed with `--confirmed`.
    Token(TokenArgs),

    /// Run the internal HTTP management API
    ///
    /// Starts a loopback-only HTTP API by default at `127.0.0.1:7878`.
    /// All endpoints except `/v1/health` and `/v1/ready` require
    /// `Authorization: Bearer <token>` for a token listed in
    /// `--tokens-file` / `OMAKURE_TOKENS_FILE` (per-token Argon2id scopes).
    /// Binding to non-loopback addresses requires `--allow-non-loopback`.
    Api(ApiArgs),

    /// Run the machine-owned node service (HTTP API + optional workers + scheduler)
    ///
    /// Starts the HTTP management API and optionally embeds queue workers and
    /// the existing schedule scanner in one process. Use `--workers 0
    /// --no-scheduler` for API-only (same auth surface as `omakure api`).
    /// `GET /v1/ready` is unauthenticated and returns minimal readiness.
    /// `GET /v1/admin/status` (scope `admin:status`) exposes readiness details
    /// and token reload health without secrets. Authenticated requests emit
    /// `omakure.http_audit` lines with `token_id` (Authorization redacted).
    /// SIGTERM/SIGINT stops HTTP first, then scheduling/claiming, then drains
    /// workers.
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

    /// Manage named environment files
    Env(EnvArgs),

    /// Inspect and explicitly manage the machine-owned node identity and trust registry
    Node(NodeArgs),

    /// Show resolved paths and environment diagnostics
    ///
    /// Prints the resolved binary path, omakure version, workspace root,
    /// scripts root, `.omakure/` directory, history directory, workspace
    /// config file, environments directory, active environment, and any
    /// known env overrides (`OMAKURE_SCRIPTS_DIR`, `OMAKURE_REPO`,
    /// `REPO`, `VERSION`). Pass `--json` for the machine-readable envelope.
    Config,

    /// Update omakure from GitHub releases
    ///
    /// Downloads the release archive for the current OS/arch and
    /// replaces the running binary in place. Also copies any scripts
    /// missing from your local scripts directory from the source
    /// archive of the target version — existing files are never
    /// overwritten. `--repo` defaults to `$OMAKURE_REPO` / `$REPO` /
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
    ///
    /// Global `--json` applies to lifecycle probes (`--status`, `--stop`,
    /// `--install`, `--uninstall`). Foreground serve and `--once` do not
    /// emit a JSON envelope.
    Serve(ServeArgs),
}

#[cfg(test)]
mod tests;
