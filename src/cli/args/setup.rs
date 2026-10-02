use clap::{Args, ValueEnum};

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Script path
    #[arg(value_name = "SCRIPT")]
    pub script: String,

    /// Inline schema JSON or `@path/to/schema.json`. When set, the new
    /// script is generated with this schema embedded between the
    /// `OMAKURE_SCHEMA_START` / `OMAKURE_SCHEMA_END` markers instead of
    /// the default placeholder template.
    #[arg(long = "schema-json")]
    pub schema_json: Option<String>,

    /// Read the script body from stdin and write it verbatim under the
    /// schema header when `--schema-json` is set. Without `--schema-json`,
    /// stdin is ignored and the default placeholder template is written.
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
    /// `$REPO` / `This-Is-NPC/omakure`.
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

#[derive(ValueEnum, Clone, Debug)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Pwsh,
}
