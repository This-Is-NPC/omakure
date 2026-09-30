use clap::Args;
use std::path::PathBuf;

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

    /// Fail with a structured error when required schema fields are missing
    /// instead of prompting on stdin or a TTY. Implied by `--json`. Does not
    /// disable prompts embedded in the script itself (for example `omakure init`
    /// templates that read optional values); for non-interactive runs pass
    /// arguments after `--` or use this flag and supply every required value.
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

    /// Direct secret field input as `FIELD=value`. The value is supplied to
    /// secret schema fields for this run and is redacted from stored args.
    #[arg(long = "secret", value_name = "FIELD=VALUE")]
    pub secrets: Vec<String>,

    /// Arguments forwarded to the script
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}
