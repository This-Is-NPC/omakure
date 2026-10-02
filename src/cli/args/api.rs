use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct TokenArgs {
    #[command(subcommand)]
    pub command: TokenCommand,
}

#[derive(Subcommand, Debug)]
pub enum TokenCommand {
    /// Generate a plaintext token, Argon2id hash, and TOML entry
    Generate(TokenGenerateArgs),
}

#[derive(Args, Debug)]
pub struct TokenGenerateArgs {
    /// Stable token id (logged/audited; never the secret)
    #[arg(long)]
    pub id: String,

    /// Scope to grant (repeatable), e.g. runs:read, scripts:read, *
    #[arg(long = "scope", required = true)]
    pub scopes: Vec<String>,

    /// Append the TOML entry to this tokens file (requires `--confirmed`)
    #[arg(long)]
    pub append: Option<std::path::PathBuf>,

    /// Confirm a destructive/automated `--append`
    #[arg(long)]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct ApiArgs {
    /// Address to bind the HTTP API server to
    #[arg(long, default_value = "127.0.0.1:7878")]
    pub bind: std::net::SocketAddr,

    /// Explicitly allow the HTTP API to bind to non-loopback addresses
    #[arg(long)]
    pub allow_non_loopback: bool,

    /// Deploy-only policy.toml (route groups + auth/node-service defaults).
    /// Overrides `OMAKURE_POLICY_FILE`. Separate from workspace omakure.toml.
    #[arg(long = "policy", env = "OMAKURE_POLICY_FILE")]
    pub policy: Option<std::path::PathBuf>,

    /// Multi-token TOML file (Argon2id hashes + per-token scopes).
    /// Overrides `OMAKURE_TOKENS_FILE`. Required unless the deploy policy
    /// sets `auth.tokens_file`.
    #[arg(long = "tokens-file", env = "OMAKURE_TOKENS_FILE")]
    pub tokens_file: Option<std::path::PathBuf>,

    /// Allowed secret provider ref for secrets:use / credentials:use,
    /// e.g. secret://prod/token or secret://prod/*; repeatable. Empty
    /// denies provider refs.
    #[arg(long = "secret-ref")]
    pub secret_refs: Vec<String>,
}
