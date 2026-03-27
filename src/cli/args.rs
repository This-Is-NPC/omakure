use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Omakure - TUI for navigating and running automation scripts.
#[derive(Parser, Debug)]
#[command(name = "omakure")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Scripts directory override
    #[arg(long, global = true)]
    pub scripts_dir: Option<PathBuf>,

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
pub struct RunArgs {
    /// Script name or path
    #[arg(value_name = "SCRIPT")]
    pub script: String,

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
