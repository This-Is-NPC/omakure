use clap::{Args, Subcommand};

// ---------------------------------------------------------------------------
// Queue subcommand
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct QueueArgs {
    #[command(subcommand)]
    pub command: QueueCommand,
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
