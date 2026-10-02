use clap::{Args, Subcommand};
use std::path::PathBuf;

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
    ///
    /// On Linux and macOS, production node configuration and state use
    /// machine-owned paths (`/etc/omakure` and `/var/lib/omakure` on Linux).
    /// A per-user binary install does not provision them; use the machine-service
    /// installer (`sudo … --install-node-service`) so those directories exist and
    /// `node serve` runs under the service account.
    Serve(NodeServeArgs),

    /// Establish a direct encrypted probe with one explicitly trusted peer
    DirectProbe(NodeDirectProbeArgs),

    /// Ask one trusted Performer to run a script it has already declared
    Cue(NodeCueArgs),

    /// Publish and deliver the signed set of scripts a fleet runs
    Baseline(NodeBaselineArgs),

    /// Explicitly initialize public config, identity, and local trust state
    ///
    /// On Linux and macOS, production paths default to machine-owned
    /// directories (`/etc/omakure/node.toml` and `/var/lib/omakure` on Linux).
    /// A per-user install does not create them; initializing as an unprivileged
    /// user against those paths fails with permission denied.
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

    /// Hold and use this node's enrollment authority
    Authority(NodeAuthorityArgs),

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

    /// Allowed secret provider ref for secrets:use. Same as `omakure api --secret-ref`.
    #[arg(long = "secret-ref")]
    pub secret_refs: Vec<String>,

    /// Node-local one-time bootstrap token file for the signed-bundle API.
    #[arg(long = "bootstrap-token-file", env = crate::operations::node::BOOTSTRAP_TOKEN_FILE_ENV)]
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
pub struct NodeAuthorityArgs {
    #[command(subcommand)]
    pub command: NodeAuthorityCommand,
}

#[derive(Subcommand, Debug)]
pub enum NodeAuthorityCommand {
    /// Create this node's enrollment authority key, refusing to replace one
    Create(NodeAuthorityCreateArgs),

    /// Report the authority this node holds, without its private half
    Show,

    /// Mint one enrollment bundle naming this node as the subject
    Issue(NodeAuthorityIssueArgs),
}

#[derive(Args, Debug)]
pub struct NodeAuthorityCreateArgs {
    /// Required. Creating an authority is a fleet-wide act.
    #[arg(long)]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct NodeAuthorityIssueArgs {
    /// The node that will apply this bundle. It is checked against that node's
    /// own identity when it does, so a bundle is useless anywhere else.
    #[arg(long = "audience")]
    pub audience: String,

    /// The role the audience will record for this node.
    #[arg(long, value_parser = ["conductor", "performer"])]
    pub role: String,

    /// A capability the audience will grant this node. Repeatable.
    #[arg(long = "capability")]
    pub capabilities: Vec<String>,

    /// How long the bundle stays valid, in seconds.
    #[arg(long = "lifetime-seconds", default_value_t = 3600)]
    pub lifetime_seconds: u64,
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

    /// Caller-supplied Cue id (32 lowercase hex). Omit to mint a new one.
    ///
    /// The same id retries the same instruction; a new id is a new run.
    #[arg(long = "cue-id")]
    pub cue_id: Option<String>,
}

#[derive(Args, Debug)]
pub struct NodeBaselineArgs {
    #[command(subcommand)]
    pub command: NodeBaselineCommand,
}

#[derive(Subcommand, Debug)]
pub enum NodeBaselineCommand {
    /// Create this node's baseline publisher key, refusing to replace one
    CreateKey,

    /// Sign the named workspace scripts as one baseline
    Publish(NodeBaselinePublishArgs),

    /// Deliver a signed baseline to one trusted Performer
    Push(NodeBaselinePushArgs),

    /// Put this node back on the one baseline retained before the current one
    ///
    /// A local operator action, not something a Conductor orders. The baseline
    /// plane carries exactly two message kinds and neither of them is "run the
    /// other version"; a remote rollback verb would hand a Conductor the power
    /// to flip a Performer between two code versions at will, which is a power
    /// the split between publishing and conducting exists to withhold. The
    /// drift status on `node health` says which machine to go and fix.
    ///
    /// Exactly one previous baseline is retained, and this is a swap: rolling
    /// back twice returns this node to where it started. The retained set is
    /// re-verified against the publishers this node names *today*, so a
    /// publisher revoked since the original install makes the rollback fail.
    Rollback(NodeBaselineRollbackArgs),
}

#[derive(Args, Debug)]
pub struct NodeBaselineRollbackArgs {
    /// Required. A rollback replaces every script the current baseline named.
    #[arg(long = "confirmed")]
    pub confirmed: bool,
}

#[derive(Args, Debug)]
pub struct NodeBaselinePublishArgs {
    /// A workspace-relative script path to include. Repeatable.
    ///
    /// A path rather than a bare name, because a baseline says *where* each
    /// script goes on every receiver; a name would leave that to whatever the
    /// receiver happened to guess.
    #[arg(long = "script", value_name = "PATH")]
    pub scripts: Vec<String>,

    /// How long the baseline stays installable, in seconds.
    #[arg(long = "lifetime-seconds", default_value_t = 3600)]
    pub lifetime_seconds: u64,

    /// Where to write the signed manifest.
    #[arg(long = "out", value_name = "PATH")]
    pub out: PathBuf,
}

#[derive(Args, Debug)]
pub struct NodeBaselinePushArgs {
    /// Expected canonical peer node ID.
    #[arg(long = "peer-node-id")]
    pub peer_node_id: String,

    /// The signed manifest produced by `node baseline publish`.
    #[arg(long = "manifest", value_name = "PATH")]
    pub manifest: PathBuf,

    /// How long to wait on the session for the Performer's answer.
    #[arg(long = "wait-seconds", default_value_t = 120)]
    pub wait_seconds: u32,
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
