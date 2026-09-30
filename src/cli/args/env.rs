use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct EnvArgs {
    #[command(subcommand)]
    pub command: EnvCommand,
}

#[derive(Subcommand, Debug)]
pub enum EnvCommand {
    /// List named environments
    List,

    /// Create a named environment from optional `KEY=value` pairs
    Create(EnvCreateArgs),

    /// Show a named environment with sensitive values redacted
    Show(EnvNameArgs),

    /// Set one `KEY=value` in a named environment
    Set(EnvSetArgs),

    /// Remove one key from a named environment
    Remove(EnvRemoveArgs),

    /// Replace a named environment with the provided `KEY=value` pairs
    Replace(EnvCreateArgs),

    /// Activate a named environment
    Activate(EnvNameArgs),

    /// Deactivate the current environment
    Deactivate,

    /// Delete a named environment
    Delete(EnvNameArgs),
}

#[derive(Args, Debug)]
pub struct EnvNameArgs {
    pub name: String,
}

#[derive(Args, Debug)]
pub struct EnvCreateArgs {
    pub name: String,

    #[arg(value_name = "KEY=VALUE")]
    pub params: Vec<String>,
}

#[derive(Args, Debug)]
pub struct EnvSetArgs {
    pub name: String,

    #[arg(value_name = "KEY=VALUE")]
    pub param: String,
}

#[derive(Args, Debug)]
pub struct EnvRemoveArgs {
    pub name: String,
    pub key: String,
}
