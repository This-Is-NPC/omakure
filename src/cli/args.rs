use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Omakure - TUI for navigating and running automation scripts.
///
/// Run `omakure` to open the TUI against the global workspace, or
/// `omakure <PATH>` (e.g. `omakure .`) to open the TUI against any
/// directory while keeping history, environments, and config global.
#[derive(Parser, Debug)]
#[command(name = "omakure")]
#[command(author, version, about, long_about = None)]
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
    #[command(visible_alias = "check")]
    Doctor,

    /// List Omaken flavors
    List,

    /// Install an Omaken flavor
    Install(OmakenInstallArgs),

    /// List available scripts
    Scripts,

    /// Show the full schema of one script
    Describe(DescribeArgs),

    /// Search the script index
    Search(SearchArgs),

    /// Query the run history
    History(HistoryArgs),

    /// Print the AI capability surface as JSON
    HelpAi,

    /// Create a new script template
    Init(InitArgs),

    /// Show resolved paths and env
    #[command(visible_alias = "env")]
    Config,

    /// Update omakure from GitHub releases
    Update(UpdateArgs),

    /// Remove the omakure binary
    Uninstall(UninstallArgs),

    /// Generate shell completion
    Completion(CompletionArgs),

    /// Manage themes
    Theme(ThemeArgs),
}

#[derive(Args, Debug)]
pub struct DescribeArgs {
    /// Script name or path
    #[arg(value_name = "SCRIPT")]
    pub script: String,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Free-text query (matches name, description, tags, fields)
    #[arg(value_name = "QUERY", default_value = "")]
    pub query: String,
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
pub struct UpdateArgs {
    /// GitHub repository (owner/name)
    #[arg(long)]
    pub repo: Option<String>,

    /// Release tag (vX.Y.Z)
    #[arg(long)]
    pub version: Option<String>,
}

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Remove the scripts directory as well
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

#[derive(Args, Debug)]
pub struct OmakenInstallArgs {
    /// Git URL of the flavor repository
    #[arg(value_name = "GIT_URL")]
    pub url: String,

    /// Override the install folder name
    #[arg(long)]
    pub name: Option<String>,
}
