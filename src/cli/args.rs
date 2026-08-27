use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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
    /// `Authorization: Bearer <token>`. Prefer `--tokens-file` /
    /// `OMAKURE_TOKENS_FILE` (per-token Argon2id scopes). Legacy
    /// `OMAKURE_API_TOKEN` still works when no tokens file is configured.
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
    /// `OVERTURE_*`, `CLOUD_MGMT_*`, `REPO`, `VERSION`). Pass `--json`
    /// for the machine-readable envelope.
    Config,

    /// Update omakure from GitHub releases
    ///
    /// Downloads the release archive for the current OS/arch and
    /// replaces the running binary in place. Also copies any scripts
    /// missing from your local scripts directory from the source
    /// archive of the target version — existing files are never
    /// overwritten. `--repo` defaults to `$OMAKURE_REPO` /
    /// `$OVERTURE_REPO` / `$CLOUD_MGMT_REPO` / `$REPO` /
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
    Serve(ServeArgs),
}

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
    /// Overrides `OMAKURE_TOKENS_FILE`. When set, process-wide
    /// `--capability` is ignored; scopes come from each token.
    #[arg(long = "tokens-file", env = "OMAKURE_TOKENS_FILE")]
    pub tokens_file: Option<std::path::PathBuf>,

    /// API capability to grant in legacy single-token mode
    /// (`OMAKURE_API_TOKEN`). Repeatable. Ignored when `--tokens-file`
    /// is set. Supported: config:read, scripts:read, env:read /
    /// envs:read, env:write / envs:write, env:activate / envs:activate,
    /// env:use / envs:use, secrets:use, secrets:read-metadata,
    /// credentials:use, runs:read, runs:write / runs:enqueue,
    /// batteries:read, batteries:write, admin:status, all.
    /// Node management uses narrow node:read, node:write, and trust:write
    /// capabilities.
    /// `all` grants every route capability but does not bypass
    /// `--secret-ref` (pass `--secret-ref '*'` for unrestricted refs).
    #[arg(long = "capability")]
    pub capabilities: Vec<String>,

    /// Allowed secret provider ref for secrets:use / credentials:use,
    /// e.g. secret://prod/token or secret://prod/*; repeatable. Empty
    /// denies provider refs.
    #[arg(long = "secret-ref")]
    pub secret_refs: Vec<String>,
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

#[derive(Args, Debug)]
pub struct EnvArgs {
    #[command(subcommand)]
    pub command: EnvCommand,
}

#[derive(Args, Debug)]
pub struct NodeArgs {
    /// Deterministic test-only node state directory override
    #[arg(long = "node-state-dir")]
    pub state_dir: Option<PathBuf>,

    /// Deterministic test-only node configuration path override
    #[arg(long = "node-config")]
    pub config_path: Option<PathBuf>,

    #[command(subcommand)]
    pub command: NodeCommand,
}

#[derive(Subcommand, Debug)]
pub enum NodeCommand {
    /// Run the machine-owned HTTP node service with optional workers and scheduler
    Serve(NodeServeArgs),

    /// Establish a direct encrypted probe with one explicitly trusted peer
    DirectProbe(NodeDirectProbeArgs),

    /// Ask one trusted Performer to run a script it has already declared
    Cue(NodeCueArgs),

    /// Explicitly initialize public config, identity, and local trust state
    Init,

    /// Inspect public node identity, redacted config, and bounded trust counts
    Status,

    /// List registered peers without audit history or private state
    Peers,

    /// Show current fleet health: presence, profile, and runner status
    Health,

    /// Show the bounded newest-first closed Signal feed: enrolled, revoked, run-completed
    Signals,

    /// Run one bounded in-memory LAN discovery scan
    Discovery(NodeDiscoveryArgs),

    /// Explicitly import and activate one manually trusted peer
    Trust(NodeTrustArgs),

    /// Request and explicitly approve or reject manual enrollment
    Enroll(NodeEnrollArgs),

    /// Update one peer's capability allow-list with confirmation and evidence
    Capabilities(NodeCapabilitiesArgs),

    /// Revoke one peer with confirmation and evidence
    Revoke(NodeRevokeArgs),

    /// Explicitly remove validated machine identity and node trust state
    Reset(NodeResetArgs),
}

#[derive(Args, Debug)]
pub struct NodeServeArgs {
    /// Address to bind the HTTP API server to; defaults to node.toml `api.bind`
    #[arg(long)]
    pub bind: Option<std::net::SocketAddr>,

    /// Optional direct transport listener address.
    #[arg(long = "direct-bind")]
    pub direct_bind: Option<std::net::SocketAddr>,

    /// Explicitly allow binding to non-loopback addresses
    #[arg(long)]
    pub allow_non_loopback: bool,

    /// Explicitly allow the direct transport to bind to non-loopback addresses
    #[arg(long = "allow-non-loopback-direct")]
    pub allow_non_loopback_direct: bool,

    /// Deploy-only policy.toml. Same as `omakure api --policy`.
    #[arg(long = "policy", env = "OMAKURE_POLICY_FILE")]
    pub policy: Option<std::path::PathBuf>,

    /// Number of embedded queue workers. `0` means API-only (no claiming).
    #[arg(long)]
    pub workers: Option<u32>,

    /// Explicitly enable the in-process schedule scanner.
    #[arg(long = "scheduler", default_value_t = false)]
    pub scheduler: bool,

    /// Disable the in-process schedule scanner.
    #[arg(
        long = "no-scheduler",
        default_value_t = false,
        conflicts_with = "scheduler"
    )]
    pub no_scheduler: bool,

    /// Only claim jobs whose actor matches this tag
    #[arg(long = "worker-actor-filter")]
    pub worker_actor_filter: Option<String>,

    /// Only claim jobs whose script path or name contains this pattern
    #[arg(long = "worker-script-filter")]
    pub worker_script_filter: Option<String>,

    /// Fail `/v1/ready` when configured workers are not alive
    #[arg(long)]
    pub readiness_requires_worker: bool,

    /// Fail `/v1/ready` when the scheduler is enabled but not alive
    #[arg(long)]
    pub readiness_requires_scheduler: bool,

    /// Fail `/v1/ready` while configured static peers are not connected
    #[arg(long)]
    pub readiness_requires_transport: bool,

    /// Multi-token TOML file. Same as `omakure api --tokens-file`.
    #[arg(long = "tokens-file", env = "OMAKURE_TOKENS_FILE")]
    pub tokens_file: Option<std::path::PathBuf>,

    /// API capability to grant in legacy single-token mode. Repeatable.
    #[arg(long = "capability")]
    pub capabilities: Vec<String>,

    /// Allowed secret provider ref for secrets:use. Same as `omakure api --secret-ref`.
    #[arg(long = "secret-ref")]
    pub secret_refs: Vec<String>,

    /// Node-local one-time bootstrap token file for the signed-bundle API.
    #[arg(long = "bootstrap-token-file", env = "OMAKURE_BOOTSTRAP_TOKEN_FILE")]
    pub bootstrap_token_file: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct NodeDirectProbeArgs {
    /// Peer direct transport address.
    #[arg(long)]
    pub endpoint: std::net::SocketAddr,

    /// Expected canonical peer node ID.
    #[arg(long = "peer-node-id")]
    pub peer_node_id: String,
}

#[derive(Args, Debug)]
pub struct NodeCueArgs {
    /// Peer direct transport address.
    #[arg(long)]
    pub endpoint: std::net::SocketAddr,

    /// Expected canonical peer node ID.
    #[arg(long = "peer-node-id")]
    pub peer_node_id: String,

    /// Script name as the Performer declared it. A path is not accepted: the
    /// Performer resolves the name against what it published, and a Cue never
    /// carries a location.
    #[arg(long)]
    pub script: String,

    /// Why this is being asked for. Recorded in the Performer's audit trail.
    #[arg(long)]
    pub reason: String,

    /// How long to stay on the session waiting for the `run-completed` Signal.
    ///
    /// The outcome is read on the connection this dial already opened, because
    /// a Performer that holds a standing session with this Conductor refuses
    /// the dial outright — the configuration that would deliver the Signal is
    /// the one in which the Cue could not be sent. `0` dispatches and returns
    /// immediately; the run still happens and still reports.
    #[arg(long = "wait-seconds", default_value_t = 120)]
    pub wait_seconds: u32,

    /// Dial the peer from this process instead of asking the running service.
    ///
    /// The service is preferred because it is the only thing that can reach a
    /// peer this node already has a session with. Use this for a peer there is
    /// no standing session with, or when no service is running.
    #[arg(long)]
    pub direct: bool,
}

#[derive(Args, Debug)]
pub struct NodeDiscoveryArgs {
    /// Discovery scan duration in seconds, bounded to 1..=30
    #[arg(long, default_value_t = 5)]
    pub wait_seconds: u64,

    /// Include observed source addresses in the local CLI result
    #[arg(long)]
    pub include_addresses: bool,
}

#[derive(Args, Debug)]
pub struct NodeResetArgs {
    /// Confirm destructive removal of identity and trust state
    #[arg(long)]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct NodeTrustArgs {
    /// Canonical omk1_ node identifier
    #[arg(long)]
    pub node_id: String,

    /// Lowercase hexadecimal x-only BIP-340 public key
    #[arg(long)]
    pub public_key: String,

    /// Signed transport certificate as lowercase hexadecimal bytes
    #[arg(long)]
    pub transport_certificate: Option<String>,

    /// Peer role: conductor or performer
    #[arg(long, default_value = "performer")]
    pub role: String,

    /// Allowed capability (repeatable; sorted unique values are required)
    #[arg(long = "capability")]
    pub capabilities: Vec<String>,

    /// Audit actor
    #[arg(long)]
    pub actor: String,

    /// Audit reason/evidence
    #[arg(long)]
    pub reason: String,

    /// Confirm this trust mutation explicitly
    #[arg(long)]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct NodeEnrollArgs {
    #[command(subcommand)]
    pub command: NodeEnrollCommand,
}

#[derive(Subcommand, Debug)]
pub enum NodeEnrollCommand {
    /// Create and send one signed manual enrollment request
    Request(NodeEnrollRequestArgs),

    /// Approve one pending request after checking the out-of-band code
    Approve(NodeEnrollApproveArgs),

    /// Reject one pending request without activating trust
    Reject(NodeEnrollRejectArgs),

    /// Apply one authority-signed unattended enrollment bundle
    Apply(NodeEnrollApplyArgs),
}

#[derive(Args, Debug)]
pub struct NodeEnrollRequestArgs {
    /// Peer direct transport address
    #[arg(long)]
    pub endpoint: std::net::SocketAddr,

    /// Requested peer role
    #[arg(long, default_value = "performer")]
    pub role: String,

    /// Requested capability (repeatable; sorted unique values are required)
    #[arg(long = "capability")]
    pub capabilities: Vec<String>,

    /// Request lifetime in seconds, at most 30 days
    #[arg(long, default_value_t = 600)]
    pub lifetime_seconds: u64,
}

#[derive(Args, Debug)]
pub struct NodeEnrollApproveArgs {
    /// Exact signed OMMA request as lowercase hexadecimal bytes
    #[arg(long = "request")]
    pub request_hex: String,

    /// Candidate transport certificate as lowercase hexadecimal bytes
    #[arg(long)]
    pub transport_certificate: String,

    /// Out-of-band 16-byte approval code as lowercase hexadecimal
    #[arg(long)]
    pub code: String,

    /// Audit actor
    #[arg(long)]
    pub actor: String,

    /// Audit reason/evidence
    #[arg(long)]
    pub reason: String,

    /// Confirm this trust mutation explicitly
    #[arg(long)]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct NodeEnrollRejectArgs {
    /// Pending candidate node identifier
    pub node_id: String,

    /// Audit actor
    #[arg(long)]
    pub actor: String,

    /// Audit reason/evidence
    #[arg(long)]
    pub reason: String,

    /// Confirm this denial explicitly
    #[arg(long)]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct NodeEnrollApplyArgs {
    /// Exact signed OMEB bundle file. The file is never echoed or persisted.
    #[arg(long = "bundle-file")]
    pub bundle_file: PathBuf,

    /// One-time bootstrap token file. The token is never echoed or persisted.
    #[arg(long = "bootstrap-token-file")]
    pub bootstrap_token_file: PathBuf,

    /// One-time 16-byte bootstrap nonce as lowercase hexadecimal.
    #[arg(long = "bootstrap-nonce")]
    pub bootstrap_nonce: String,
}

#[derive(Args, Debug)]
pub struct NodeCapabilitiesArgs {
    /// Peer node identifier
    pub node_id: String,

    /// Allowed capability (repeatable; sorted unique values are required)
    #[arg(long = "capability")]
    pub capabilities: Vec<String>,

    /// Audit actor
    #[arg(long)]
    pub actor: String,

    /// Audit reason/evidence
    #[arg(long)]
    pub reason: String,

    /// Confirm this trust mutation explicitly
    #[arg(long)]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct NodeRevokeArgs {
    /// Peer node identifier
    pub node_id: String,

    /// Audit actor
    #[arg(long)]
    pub actor: String,

    /// Audit reason/evidence
    #[arg(long)]
    pub reason: String,

    /// Confirm this trust mutation explicitly
    #[arg(long)]
    pub confirmed: bool,
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
#[command(disable_version_flag = true)]
pub struct UpdateArgs {
    /// GitHub repository (`owner/name`). Defaults to `$OMAKURE_REPO` /
    /// `$OVERTURE_REPO` / `$CLOUD_MGMT_REPO` / `$REPO` /
    /// `This-Is-NPC/omakure`.
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

// ---------------------------------------------------------------------------
// Queue subcommand
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct QueueArgs {
    #[command(subcommand)]
    pub command: QueueCommand,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("omakure").chain(args.iter().copied()))
    }

    #[test]
    fn test_parse_no_args_leaves_command_empty_for_help() {
        let cli = parse(&[]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_parse_run_subcommand() {
        let cli = parse(&["run", "deploy.sh"]).unwrap();
        match cli.command.unwrap() {
            Commands::Run(args) => {
                assert_eq!(args.script, "deploy.sh");
                assert_eq!(args.actor, "human");
                assert!(!args.no_prompt);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn test_parse_run_with_flags() {
        let cli = parse(&["run", "deploy.sh", "--json", "--no-prompt", "--actor", "ai"]).unwrap();
        assert!(cli.json);
        match cli.command.unwrap() {
            Commands::Run(args) => {
                assert_eq!(args.actor, "ai");
                assert!(args.no_prompt);
            }
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn test_parse_run_secret_input() {
        let cli = parse(&["run", "deploy.sh", "--secret", "TOKEN=direct"]);
        let cli = cli.unwrap();
        match cli.command.unwrap() {
            Commands::Run(args) => assert_eq!(args.secrets, vec!["TOKEN=direct"]),
            _ => panic!("expected Run command"),
        }
    }

    #[test]
    fn test_parse_env_namespace() {
        let cli = parse(&["env", "set", "prod", "API_KEY=secret"]);
        let cli = cli.unwrap();
        match cli.command.unwrap() {
            Commands::Env(args) => match args.command {
                EnvCommand::Set(set) => {
                    assert_eq!(set.name, "prod");
                    assert_eq!(set.param, "API_KEY=secret");
                }
                _ => panic!("expected env set"),
            },
            _ => panic!("expected Env command"),
        }
    }

    #[test]
    fn test_parse_global_json_flag() {
        let cli = parse(&["--json", "scripts"]).unwrap();
        assert!(cli.json);
    }

    #[test]
    fn test_unknown_top_level_command_is_rejected() {
        assert!(parse(&["list"]).is_err());
    }

    #[test]
    fn test_omaken_install_command_is_removed() {
        let result = parse(&["install", "https://example.com/scripts.git"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_queue_add() {
        let cli = parse(&["queue", "add", "deploy.sh", "--priority", "10"]).unwrap();
        match cli.command.unwrap() {
            Commands::Queue(q) => match q.command {
                QueueCommand::Add(args) => {
                    assert_eq!(args.script, "deploy.sh");
                    assert_eq!(args.priority, 10);
                }
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Queue"),
        }
    }

    #[test]
    fn test_parse_battery_add() {
        let cli = parse(&[
            "battery",
            "add",
            "https://example.invalid/azure.git",
            "--name",
            "azure",
            "--ref",
            "stable",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Commands::Battery(args) => match args.command {
                BatteryCommand::Add(add) => {
                    assert_eq!(add.git_url, "https://example.invalid/azure.git");
                    assert_eq!(add.name, "azure");
                    assert_eq!(add.requested_ref, "stable");
                }
                _ => panic!("expected Battery Add"),
            },
            _ => panic!("expected Battery"),
        }
    }

    #[test]
    fn test_parse_battery_install_force() {
        let cli = parse(&["battery", "install", "azure", "azure.list", "--force"]).unwrap();
        match cli.command.unwrap() {
            Commands::Battery(args) => match args.command {
                BatteryCommand::Install(install) => {
                    assert_eq!(install.name, "azure");
                    assert_eq!(install.script_id, "azure.list");
                    assert!(install.force);
                }
                _ => panic!("expected Battery Install"),
            },
            _ => panic!("expected Battery"),
        }
    }

    #[test]
    fn test_parse_battery_remove_cache() {
        let cli = parse(&["battery", "remove", "azure", "--remove-cache"]).unwrap();
        match cli.command.unwrap() {
            Commands::Battery(args) => match args.command {
                BatteryCommand::Remove(remove) => {
                    assert_eq!(remove.name, "azure");
                    assert!(remove.remove_cache);
                }
                _ => panic!("expected Battery Remove"),
            },
            _ => panic!("expected Battery"),
        }
    }

    #[test]
    fn test_parse_api_defaults() {
        let cli = parse(&["api"]).unwrap();
        match cli.command.unwrap() {
            Commands::Api(args) => {
                assert_eq!(args.bind.to_string(), "127.0.0.1:7878");
                assert!(!args.allow_non_loopback);
                assert!(args.tokens_file.is_none());
            }
            _ => panic!("expected Api"),
        }
    }

    #[test]
    fn test_parse_token_generate() {
        let cli = parse(&[
            "token",
            "generate",
            "--id",
            "ci",
            "--scope",
            "runs:read",
            "--scope",
            "scripts:read",
            "--append",
            "/tmp/tokens.toml",
            "--confirmed",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Commands::Token(args) => match args.command {
                TokenCommand::Generate(g) => {
                    assert_eq!(g.id, "ci");
                    assert_eq!(g.scopes, vec!["runs:read", "scripts:read"]);
                    assert_eq!(
                        g.append.as_deref(),
                        Some(std::path::Path::new("/tmp/tokens.toml"))
                    );
                    assert!(g.confirmed);
                }
            },
            _ => panic!("expected Token"),
        }
    }

    #[test]
    fn test_parse_api_bind_flags() {
        let cli = parse(&["api", "--bind", "0.0.0.0:7878", "--allow-non-loopback"]).unwrap();
        match cli.command.unwrap() {
            Commands::Api(args) => {
                assert_eq!(args.bind.to_string(), "0.0.0.0:7878");
                assert!(args.allow_non_loopback);
            }
            _ => panic!("expected Api"),
        }
    }

    #[test]
    fn test_parse_api_policy_flag() {
        let cli = parse(&["api", "--policy", "/etc/omakure/policy.toml"]).unwrap();
        match cli.command.unwrap() {
            Commands::Api(args) => {
                assert_eq!(
                    args.policy.as_deref(),
                    Some(std::path::Path::new("/etc/omakure/policy.toml"))
                );
            }
            _ => panic!("expected Api"),
        }
    }

    #[test]
    fn test_api_help_surface_exists() {
        let command = Cli::command();
        let api = command
            .find_subcommand("api")
            .expect("api subcommand should be registered");
        assert!(api.get_arguments().any(|arg| arg.get_id() == "bind"));
        assert!(api
            .get_arguments()
            .any(|arg| arg.get_id() == "allow_non_loopback"));
    }

    #[test]
    fn test_parse_node_serve_flags() {
        let cli = parse(&[
            "node",
            "serve",
            "--bind",
            "127.0.0.1:8787",
            "--workers",
            "2",
            "--scheduler",
            "--readiness-requires-worker",
            "--readiness-requires-scheduler",
            "--allow-non-loopback-direct",
            "--worker-actor-filter",
            "agent",
            "--worker-script-filter",
            "tools/",
            "--capability",
            "runs:write",
            "--secret-ref",
            "secret://env/*",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Commands::Node(args) => match args.command {
                NodeCommand::Serve(args) => {
                    assert_eq!(args.bind.unwrap().to_string(), "127.0.0.1:8787");
                    assert_eq!(args.workers, Some(2));
                    assert!(args.scheduler);
                    assert!(!args.no_scheduler);
                    assert!(args.readiness_requires_worker);
                    assert!(args.readiness_requires_scheduler);
                    assert!(args.allow_non_loopback_direct);
                    assert_eq!(args.worker_actor_filter.as_deref(), Some("agent"));
                    assert_eq!(args.worker_script_filter.as_deref(), Some("tools/"));
                    assert_eq!(args.capabilities, vec!["runs:write".to_string()]);
                    assert_eq!(args.secret_refs, vec!["secret://env/*".to_string()]);
                }
                _ => panic!("expected node serve"),
            },
            _ => panic!("expected Node"),
        }
    }

    #[test]
    fn node_serve_direct_non_loopback_flag_is_separate_from_http_flag() {
        let cli = parse(&["node", "serve", "--allow-non-loopback-direct"]).unwrap();
        match cli.command.unwrap() {
            Commands::Node(args) => match args.command {
                NodeCommand::Serve(args) => {
                    assert!(args.allow_non_loopback_direct);
                    assert!(!args.allow_non_loopback);
                }
                _ => panic!("expected node serve"),
            },
            _ => panic!("expected Node"),
        }
    }

    #[test]
    fn test_node_reset_requires_explicit_flag_in_surface() {
        let cli = parse(&["node", "reset", "--confirmed"]).unwrap();
        match cli.command.unwrap() {
            Commands::Node(args) => match args.command {
                NodeCommand::Reset(args) => assert!(args.confirmed),
                _ => panic!("expected node reset"),
            },
            _ => panic!("expected Node"),
        }
    }

    #[test]
    fn test_node_serve_defaults_are_safe() {
        let cli = parse(&["node", "serve"]).unwrap();
        match cli.command.unwrap() {
            Commands::Node(args) => match args.command {
                NodeCommand::Serve(args) => {
                    assert!(args.bind.is_none());
                    assert_eq!(args.workers, None);
                    assert!(!args.scheduler);
                    assert!(!args.no_scheduler);
                    assert!(!args.readiness_requires_worker);
                    assert!(!args.readiness_requires_scheduler);
                    assert!(args.capabilities.is_empty());
                    assert!(args.secret_refs.is_empty());
                }
                _ => panic!("expected node serve"),
            },
            _ => panic!("expected Node"),
        }
    }

    #[test]
    fn test_node_serve_help_surface_exists() {
        let command = Cli::command();
        let node = command
            .find_subcommand("node")
            .expect("node subcommand should be registered");
        let serve = node
            .find_subcommand("serve")
            .expect("node serve should be registered");
        assert!(serve.get_arguments().any(|arg| arg.get_id() == "workers"));
        assert!(serve
            .get_arguments()
            .any(|arg| arg.get_id() == "readiness_requires_worker"));
    }

    #[test]
    fn test_parse_node_serve_policy_flag() {
        let cli = parse(&["node", "serve", "--policy", "/tmp/p.toml"]).unwrap();
        match cli.command.unwrap() {
            Commands::Node(args) => match args.command {
                NodeCommand::Serve(args) => assert_eq!(
                    args.policy.as_deref(),
                    Some(std::path::Path::new("/tmp/p.toml"))
                ),
                _ => panic!("expected node serve"),
            },
            _ => panic!("expected Node"),
        }
    }

    #[test]
    fn test_parse_queue_worker() {
        let cli = parse(&["queue", "worker", "--concurrency", "4"]).unwrap();
        match cli.command.unwrap() {
            Commands::Queue(q) => match q.command {
                QueueCommand::Worker(args) => assert_eq!(args.concurrency, 4),
                _ => panic!("expected Worker"),
            },
            _ => panic!("expected Queue"),
        }
    }

    #[test]
    fn test_parse_history_list_with_state() {
        let cli = parse(&["history", "list", "--state", "completed", "--since", "1h"]).unwrap();
        match cli.command.unwrap() {
            Commands::History(h) => match h.command {
                HistoryCommand::List(args) => {
                    assert_eq!(args.state, vec!["completed"]);
                    assert_eq!(args.since, Some("1h".to_string()));
                }
                _ => panic!("expected List"),
            },
            _ => panic!("expected History"),
        }
    }

    #[test]
    fn test_parse_history_show() {
        let cli = parse(&["history", "show", "abc123"]).unwrap();
        match cli.command.unwrap() {
            Commands::History(h) => match h.command {
                HistoryCommand::Show(args) => assert_eq!(args.run_id, "abc123"),
                _ => panic!("expected Show"),
            },
            _ => panic!("expected History"),
        }
    }

    #[test]
    fn test_conflicting_scripts_dir_and_path() {
        let result = parse(&["--scripts-dir", "/a", "/b"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_describe() {
        let cli = parse(&["describe", "deploy.sh"]).unwrap();
        match cli.command.unwrap() {
            Commands::Describe(args) => assert_eq!(args.script, "deploy.sh"),
            _ => panic!("expected Describe"),
        }
    }

    #[test]
    fn test_parse_search() {
        let cli = parse(&["search", "deploy", "--tag", "infra"]).unwrap();
        match cli.command.unwrap() {
            Commands::Search(args) => {
                assert_eq!(args.query, "deploy");
                assert_eq!(args.tag, vec!["infra"]);
            }
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn test_parse_init_with_schema() {
        let cli = parse(&["init", "new.sh", "--schema-json", "{}"]).unwrap();
        match cli.command.unwrap() {
            Commands::Init(args) => {
                assert_eq!(args.script, Some("new.sh".to_string()));
                assert_eq!(args.schema_json, Some("{}".to_string()));
            }
            _ => panic!("expected Init"),
        }
    }

    #[test]
    fn test_parse_trace() {
        let cli = parse(&["trace", "hello", "--level", "warn", "--data", "{\"k\":1}"]).unwrap();
        match cli.command.unwrap() {
            Commands::Trace(args) => {
                assert_eq!(args.message, "hello");
                assert_eq!(args.level, "warn");
                assert_eq!(args.data, Some("{\"k\":1}".to_string()));
            }
            _ => panic!("expected Trace"),
        }
    }

    #[test]
    fn test_history_success_failure_conflict() {
        let result = parse(&["history", "list", "--success", "--failure"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_history_state_and_state_set_conflict() {
        let result = parse(&[
            "history",
            "list",
            "--state",
            "completed",
            "--state-set",
            "all",
        ]);
        assert!(result.is_err());
    }
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

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Run the scheduler as a detached background daemon (Unix only).
    #[arg(long, short = 'd', conflicts_with_all = ["stop", "install", "uninstall", "status"])]
    pub detach: bool,

    /// Stop a running daemon (reads `.omakure/daemon.pid` and sends SIGTERM).
    #[arg(long, conflicts_with_all = ["install", "uninstall", "status"])]
    pub stop: bool,

    /// Install a systemd user service that runs `omakure serve` for the
    /// current workspace and survives reboots (Linux only).
    #[arg(long, conflicts_with_all = ["uninstall", "status"])]
    pub install: bool,

    /// Disable and remove the systemd user service for the current
    /// workspace (Linux only).
    #[arg(long, conflicts_with_all = ["status"])]
    pub uninstall: bool,

    /// Print the systemd user service status for the current workspace
    /// (Linux only).
    #[arg(long)]
    pub status: bool,

    /// Do not spawn the in-process worker. Use when you already run
    /// `omakure queue worker` elsewhere.
    #[arg(long = "no-worker")]
    pub no_worker: bool,

    /// Number of worker threads for the in-process worker (default 1).
    #[arg(long, default_value_t = 1)]
    pub concurrency: u32,

    /// Test convenience: run a single scheduler tick, enqueue whatever is
    /// due, then exit. Hidden from --help. Used by integration tests.
    #[arg(long, hide = true)]
    pub once: bool,
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
