use clap::{Args, Subcommand};

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

    /// Start and inspect installed Battery workflows
    Workflow(BatteryWorkflowArgs),
}

#[derive(Args, Debug)]
pub struct BatteryWorkflowArgs {
    #[command(subcommand)]
    pub command: BatteryWorkflowCommand,
}

#[derive(Subcommand, Debug)]
pub enum BatteryWorkflowCommand {
    /// Start a workflow using installed scripts from one Battery
    Start(BatteryWorkflowStartArgs),

    /// Inspect a durable workflow and its step results
    Status(BatteryWorkflowStatusArgs),
}

#[derive(Args, Debug)]
pub struct BatteryWorkflowStartArgs {
    /// Registered Battery name
    #[arg(value_name = "BATTERY")]
    pub battery_name: String,

    /// Workflow id from the Battery manifest
    #[arg(value_name = "WORKFLOW")]
    pub workflow_name: String,
}

#[derive(Args, Debug)]
pub struct BatteryWorkflowStatusArgs {
    /// Workflow run id returned by `battery workflow start`
    #[arg(value_name = "WORKFLOW_RUN_ID")]
    pub workflow_run_id: String,
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

    /// Secret ref for private HTTPS auth (`secret://provider/key`).
    /// Registry stores the ref only; sync resolves via GIT_ASKPASS.
    #[arg(long = "token-ref")]
    pub token_ref: Option<String>,
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
