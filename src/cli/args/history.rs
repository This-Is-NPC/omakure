use clap::{Args, Subcommand};

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

    /// Unsupported; rejected with error.code = "not_implemented"
    #[arg(long)]
    pub follow: bool,
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
