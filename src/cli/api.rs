use crate::auth::{self, AuthContext, Authenticator};
use crate::cli::args::ApiArgs;
use crate::cli::json;
use crate::operations::battery as battery_ops;
use crate::operations::config as config_ops;
use crate::operations::core;
use crate::operations::doctor as doctor_ops;
use crate::operations::envs as env_ops;
use crate::operations::node as node_ops;
use crate::operations::scripts as scripts_ops;
use crate::operations::search as search_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::policy::{self, DeployPolicy};
use crate::ports::ScriptRepository;
use crate::workspace::Workspace;
use axum::body::{to_bytes, Body};
use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Extension;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

/// Structured HTTP audit event. Never includes Authorization or raw tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HttpAuditEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub method: String,
    pub path: String,
    pub outcome: String,
    pub status: u16,
}

#[derive(Debug, Clone)]
struct AuditRunId(String);

type AuditHook = Arc<dyn Fn(&HttpAuditEvent) + Send + Sync>;

fn audit_hook_slot() -> &'static RwLock<Option<AuditHook>> {
    static SLOT: OnceLock<RwLock<Option<AuditHook>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

fn emit_http_audit(event: HttpAuditEvent) {
    if let Ok(guard) = audit_hook_slot().read() {
        if let Some(hook) = guard.as_ref() {
            hook(&event);
        }
    }
    if let Ok(line) = serde_json::to_string(&event) {
        // Operators correlate enqueue/cancel/dead-letter via token_id in this line.
        eprintln!("omakure.http_audit {line}");
    }
}

#[cfg(test)]
fn install_audit_hook(hook: AuditHook) {
    *audit_hook_slot().write().expect("audit hook lock") = Some(hook);
}

#[cfg(test)]
fn clear_audit_hook() {
    *audit_hook_slot().write().expect("audit hook lock") = None;
}

#[cfg(test)]
const BODY_LIMIT_BYTES: usize = 1024 * 1024;
const MAX_SEARCH_QUERY_LEN: usize = 256;
const MAX_SEARCH_TAGS: usize = 16;
const MAX_SEARCH_TAG_LEN: usize = 64;

/// Shared readiness gate for `GET /v1/ready`.
///
/// Minimal by design: callers only learn whether the process is ready, never
/// token IDs, paths, or secret metadata.
#[derive(Debug)]
pub(crate) struct ReadinessGate {
    pub requires_worker: bool,
    pub requires_scheduler: bool,
    pub workers_configured: bool,
    pub scheduler_configured: bool,
    pub workers_alive: AtomicBool,
    pub scheduler_alive: AtomicBool,
}

impl ReadinessGate {
    pub(crate) fn new(
        requires_worker: bool,
        requires_scheduler: bool,
        workers_configured: bool,
        scheduler_configured: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            requires_worker,
            requires_scheduler,
            workers_configured,
            scheduler_configured,
            workers_alive: AtomicBool::new(false),
            scheduler_alive: AtomicBool::new(false),
        })
    }

    pub(crate) fn is_ready(&self) -> bool {
        if self.requires_worker
            && self.workers_configured
            && !self.workers_alive.load(Ordering::SeqCst)
        {
            return false;
        }
        if self.requires_scheduler
            && self.scheduler_configured
            && !self.scheduler_alive.load(Ordering::SeqCst)
        {
            return false;
        }
        true
    }

    pub(crate) fn set_workers_alive(&self, alive: bool) {
        self.workers_alive.store(alive, Ordering::SeqCst);
    }

    pub(crate) fn set_scheduler_alive(&self, alive: bool) {
        self.scheduler_alive.store(alive, Ordering::SeqCst);
    }
}

struct ApiState {
    auth: Authenticator,
    workspace: Workspace,
    /// Process-wide capabilities (legacy mode) + secret-ref ACL.
    policy: ApiPolicy,
    /// Deploy-time route-group gates (before scopes).
    deploy: DeployPolicy,
    readiness: Option<Arc<ReadinessGate>>,
    auth_verification_gate: Arc<tokio::sync::Semaphore>,
}

impl Clone for ApiState {
    fn clone(&self) -> Self {
        Self {
            auth: self.auth.clone(),
            workspace: self.workspace.clone_for_executor(),
            policy: self.policy.clone(),
            deploy: self.deploy.clone(),
            readiness: self.readiness.clone(),
            auth_verification_gate: Arc::clone(&self.auth_verification_gate),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiCapability {
    ConfigRead,
    ScriptsRead,
    EnvRead,
    EnvWrite,
    EnvActivate,
    EnvUse,
    SecretProviderUse,
    SecretsReadMetadata,
    CredentialsUse,
    RunRead,
    RunWrite,
    BatteryRead,
    BatteryWrite,
    AdminStatus,
    NodeRead,
    NodeWrite,
    TrustWrite,
}

#[derive(Debug, Clone)]
pub(crate) struct ApiPolicy {
    capabilities: Vec<ApiCapability>,
    allowed_secret_refs: Option<Vec<String>>,
}

impl ApiPolicy {
    fn all() -> Self {
        let mut policy = Self::allow([
            ApiCapability::ConfigRead,
            ApiCapability::ScriptsRead,
            ApiCapability::EnvRead,
            ApiCapability::EnvWrite,
            ApiCapability::EnvActivate,
            ApiCapability::EnvUse,
            ApiCapability::SecretProviderUse,
            ApiCapability::SecretsReadMetadata,
            ApiCapability::CredentialsUse,
            ApiCapability::RunRead,
            ApiCapability::RunWrite,
            ApiCapability::BatteryRead,
            ApiCapability::BatteryWrite,
            ApiCapability::AdminStatus,
            ApiCapability::NodeRead,
            ApiCapability::NodeWrite,
            ApiCapability::TrustWrite,
        ]);
        // Explicit empty allow-list: `all` does not bypass secret-ref ACLs.
        // Grant refs with repeated `--secret-ref` (or leave empty to deny provider refs).
        policy.allowed_secret_refs = Some(Vec::new());
        policy
    }

    fn allow<const N: usize>(capabilities: [ApiCapability; N]) -> Self {
        Self {
            capabilities: capabilities.into(),
            allowed_secret_refs: Some(Vec::new()),
        }
    }

    fn from_config(capabilities: &[String], refs: &[String]) -> Result<Self, ApiConfigError> {
        // Normalize operator ref spellings (e.g. `secret://env:NAME`) to the
        // canonical form the ACL is compared against, so the colon form is not
        // silently dropped.
        let refs: Vec<String> = refs
            .iter()
            .map(|r| crate::secrets::canonicalize_operator_secret_ref(r))
            .collect();
        let mut parsed = Vec::new();
        for capability in capabilities {
            if capability == "all" {
                // `all` expands route capabilities only — secret-ref ACL still
                // comes from `--secret-ref` (empty denies provider refs).
                let mut policy = Self::all();
                policy.allowed_secret_refs = Some(refs.clone());
                return Ok(policy);
            }
            parsed.push(ApiCapability::from_config_value(capability)?);
        }
        Ok(Self {
            capabilities: parsed,
            allowed_secret_refs: Some(refs),
        })
    }

    #[cfg(test)]
    fn allow_with_secret_refs<const N: usize, const M: usize>(
        capabilities: [ApiCapability; N],
        refs: [&str; M],
    ) -> Self {
        Self {
            capabilities: capabilities.into(),
            allowed_secret_refs: Some(refs.into_iter().map(str::to_string).collect()),
        }
    }

    fn permits(&self, capability: ApiCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    fn secret_access(&self, auth: &AuthContext, file_mode: bool) -> crate::secrets::SecretAccess {
        let mut scopes = Vec::new();
        if file_mode {
            if auth.has_scope("secrets:use") {
                scopes.push("secrets:use");
            }
            if auth.has_scope("credentials:use") {
                scopes.push("credentials:use");
            }
            if auth.has_scope("secrets:read-metadata") || auth.has_scope("*") {
                scopes.push("secrets:read-metadata");
            }
        } else {
            if self.permits(ApiCapability::SecretProviderUse) {
                scopes.push("secrets:use");
            }
            if self.permits(ApiCapability::CredentialsUse) {
                scopes.push("credentials:use");
            }
            if self.permits(ApiCapability::SecretsReadMetadata) {
                scopes.push("secrets:read-metadata");
            }
        }
        match &self.allowed_secret_refs {
            // None is treated as deny-all refs (same as empty list). Unrestricted
            // secret refs require an explicit `--secret-ref '*'` / provider wildcard.
            None => crate::secrets::SecretAccess::new(scopes, Vec::<String>::new()),
            Some(refs) => {
                if refs.iter().any(|r| r == "*") {
                    // Wildcard grants every file/provider ref but keeps env refs
                    // gated behind explicitly listed `secret://env/...` entries.
                    let env_refs = refs.iter().filter(|r| *r != "*").cloned();
                    crate::secrets::SecretAccess::allow_all_non_env(scopes, env_refs)
                } else {
                    crate::secrets::SecretAccess::new(scopes, refs.iter().cloned())
                }
            }
        }
    }

    /// Secret ACL for Battery HTTPS token_ref (requires credentials:use).
    fn battery_credential_access(
        &self,
        auth: &AuthContext,
        file_mode: bool,
    ) -> crate::secrets::SecretAccess {
        let has_credentials = if file_mode {
            auth.has_scope("credentials:use") || auth.has_scope("*")
        } else {
            self.permits(ApiCapability::CredentialsUse)
        };
        if !has_credentials {
            return crate::secrets::SecretAccess::new(Vec::<&str>::new(), Vec::<String>::new());
        }
        match &self.allowed_secret_refs {
            None => crate::secrets::SecretAccess::new(["credentials:use"], Vec::<String>::new()),
            Some(refs) => {
                if refs.iter().any(|r| r == "*") {
                    let env_refs = refs.iter().filter(|r| *r != "*").cloned();
                    crate::secrets::SecretAccess::allow_all_non_env(["credentials:use"], env_refs)
                } else {
                    crate::secrets::SecretAccess::new(["credentials:use"], refs.iter().cloned())
                }
            }
        }
    }
}

impl ApiCapability {
    fn from_config_value(value: &str) -> Result<Self, ApiConfigError> {
        match value {
            "config:read" => Ok(Self::ConfigRead),
            "scripts:read" => Ok(Self::ScriptsRead),
            "env:read" | "envs:read" => Ok(Self::EnvRead),
            "env:write" | "envs:write" => Ok(Self::EnvWrite),
            "env:activate" | "envs:activate" => Ok(Self::EnvActivate),
            "env:use" | "envs:use" => Ok(Self::EnvUse),
            "secrets:use" => Ok(Self::SecretProviderUse),
            "secrets:read-metadata" => Ok(Self::SecretsReadMetadata),
            "credentials:use" => Ok(Self::CredentialsUse),
            "runs:read" => Ok(Self::RunRead),
            "runs:write" | "runs:enqueue" | "runs:cancel" | "runs:dead-letter" => {
                Ok(Self::RunWrite)
            }
            "batteries:read" => Ok(Self::BatteryRead),
            "batteries:write" | "batteries:add" | "batteries:sync" | "batteries:install"
            | "batteries:remove" => Ok(Self::BatteryWrite),
            "admin:status" => Ok(Self::AdminStatus),
            "node:read" => Ok(Self::NodeRead),
            "node:write" => Ok(Self::NodeWrite),
            "trust:write" => Ok(Self::TrustWrite),
            "doctor:read" | "workspace:read" => Ok(Self::ConfigRead),
            "search:read" => Ok(Self::ScriptsRead),
            _ => Err(ApiConfigError::InvalidCapability(value.to_string())),
        }
    }

    fn as_scope(&self) -> &'static str {
        match self {
            Self::ConfigRead => "config:read",
            Self::ScriptsRead => "scripts:read",
            Self::EnvRead => "envs:read",
            Self::EnvWrite => "envs:write",
            Self::EnvActivate => "envs:activate",
            Self::EnvUse => "envs:use",
            Self::SecretProviderUse => "secrets:use",
            Self::SecretsReadMetadata => "secrets:read-metadata",
            Self::CredentialsUse => "credentials:use",
            Self::RunRead => "runs:read",
            Self::RunWrite => "runs:write",
            Self::BatteryRead => "batteries:read",
            Self::BatteryWrite => "batteries:write",
            Self::AdminStatus => "admin:status",
            Self::NodeRead => "node:read",
            Self::NodeWrite => "node:write",
            Self::TrustWrite => "trust:write",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApiConfigError {
    MissingToken,
    InvalidToken,
    NonLoopbackBind(SocketAddr),
    InvalidCapability(String),
    Auth(String),
    Policy(String),
}

impl std::fmt::Display for ApiConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingToken => write!(
                f,
                "auth required: set OMAKURE_TOKENS_FILE/--tokens-file or OMAKURE_API_TOKEN"
            ),
            Self::InvalidToken => write!(f, "OMAKURE_API_TOKEN is invalid"),
            Self::NonLoopbackBind(addr) => write!(
                f,
                "refusing to bind {addr}; pass --allow-non-loopback to opt in"
            ),
            Self::InvalidCapability(value) => write!(f, "invalid API capability: {value}"),
            Self::Auth(msg) => write!(f, "{msg}"),
            Self::Policy(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for ApiConfigError {}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct AdminStatusResponse {
    ready: bool,
    readiness: AdminReadinessDetails,
    auth: auth::AuthStatus,
}

#[derive(Serialize)]
struct AdminReadinessDetails {
    requires_worker: bool,
    requires_scheduler: bool,
    workers_configured: bool,
    scheduler_configured: bool,
    workers_alive: bool,
    scheduler_alive: bool,
}

#[derive(Debug, Deserialize)]
struct EnqueueRunBody {
    script: String,
    #[serde(default)]
    args: Vec<String>,
    env: Option<String>,
    #[serde(default)]
    secret_fields: HashMap<String, String>,
    run_id: Option<String>,
    #[serde(default = "default_actor")]
    actor: String,
    reason: Option<String>,
    #[serde(default)]
    priority: i64,
    timeout_ms: Option<i64>,
    parent_run_id: Option<String>,
    cron_schedule_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RunReasonBody {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnvBody {
    name: Option<String>,
    #[serde(default)]
    params: Vec<env_ops::EnvParam>,
}

#[derive(Debug, Deserialize)]
struct EnvParamBody {
    value: String,
}

#[derive(Debug, Deserialize)]
struct AddBatteryBody {
    name: String,
    git_url: String,
    #[serde(default = "default_battery_ref")]
    requested_ref: String,
    /// Optional `secret://provider/key` for private HTTPS Battery auth.
    #[serde(default)]
    token_ref: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct InstallBatteryScriptBody {
    #[serde(default)]
    force: bool,
}

fn default_actor() -> String {
    "human".to_string()
}

fn default_battery_ref() -> String {
    "main".to_string()
}

#[derive(Debug, Default, Deserialize)]
struct NodeInitializeBody {}

#[derive(Debug, Deserialize)]
struct ManualTrustBody {
    node_id: String,
    public_key: String,
    role: String,
    #[serde(default)]
    capabilities: Vec<String>,
    actor: String,
    reason: String,
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct NodeCapabilitiesBody {
    #[serde(default)]
    capabilities: Vec<String>,
    actor: String,
    reason: String,
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct NodeRevokeBody {
    actor: String,
    reason: String,
    confirmed: bool,
}

pub fn run(scripts_dir: PathBuf, args: ApiArgs) -> Result<(), Box<dyn Error>> {
    let boot = prepare_api_boot(&args)?;
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let cancel_flag = Arc::new(AtomicBool::new(false));
    crate::cli::queue::install_signal_handlers(Arc::clone(&cancel_flag));
    if boot.auth.is_file_mode() {
        auth::install_sighup_reload(boot.auth.clone());
    }

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        serve_http(
            boot.bind,
            boot.auth,
            workspace,
            boot.api_policy,
            boot.deploy,
            None,
            cancel_flag,
            None,
        )
        .await
    })
}

/// Resolved startup config for `api` / `node serve` (validated before bind).
#[derive(Clone)]
pub(crate) struct ApiBoot {
    pub bind: SocketAddr,
    #[allow(dead_code)] // surfaced for callers / tests inspecting boot
    pub allow_non_loopback: bool,
    pub auth: Authenticator,
    pub api_policy: ApiPolicy,
    pub deploy: DeployPolicy,
}

/// Load deploy policy, resolve auth, and validate bind — all before any socket.
pub(crate) fn prepare_api_boot(args: &ApiArgs) -> Result<ApiBoot, ApiConfigError> {
    let env_policy = std::env::var("OMAKURE_POLICY_FILE").ok();
    let policy_path = policy::resolve_policy_path(args.policy.as_deref(), env_policy.as_deref());
    let deploy = policy::load_policy(policy_path.as_deref())
        .map_err(|e| ApiConfigError::Policy(e.to_string()))?;

    let allow_non_loopback = args.allow_non_loopback || deploy.http.allow_non_loopback;
    // CLI `--bind` wins when not the clap default; otherwise policy `http.bind`
    // (if set) overlays the default.
    let default_bind: SocketAddr = "127.0.0.1:7878".parse().expect("static bind");
    let bind = if args.bind != default_bind {
        args.bind
    } else {
        deploy.http.bind.unwrap_or(args.bind)
    };

    validate_bind(bind, allow_non_loopback)?;

    let tokens_file = args
        .tokens_file
        .clone()
        .or_else(|| deploy.auth.tokens_file.clone());
    let auth = resolve_auth_with_policy(tokens_file.as_deref(), deploy.auth.legacy_env_token)?;
    let api_policy = ApiPolicy::from_config(&args.capabilities, &args.secret_refs)?;

    Ok(ApiBoot {
        bind,
        allow_non_loopback,
        auth,
        api_policy,
        deploy,
    })
}

/// Serve the HTTP management API until `cancel_flag` is set, then shut down
/// gracefully. Used by `omakure api` and `omakure node serve`.
// Audit note: keeping the independently configured security and lifecycle
// controls explicit here is clearer than hiding them in a second config type.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve_http(
    bind: SocketAddr,
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
    deploy: DeployPolicy,
    readiness: Option<Arc<ReadinessGate>>,
    cancel_flag: Arc<AtomicBool>,
    on_listening: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<(), Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    if let Some(tx) = on_listening {
        let _ = tx.send(());
    }
    let body_limit = deploy.http.body_limit_bytes.max(1);
    let app = router_with_policy(auth, workspace, policy, deploy, readiness, body_limit);
    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_cancel(cancel_flag))
        .await?;
    Ok(())
}

async fn wait_for_cancel(cancel_flag: Arc<AtomicBool>) {
    while !cancel_flag.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
fn router(token: String, workspace: Workspace) -> Router {
    // Test convenience: full route capabilities plus unrestricted secret refs.
    // Production `--capability all` still requires explicit `--secret-ref`.
    let mut policy = ApiPolicy::all();
    policy.allowed_secret_refs = Some(vec!["*".into()]);
    router_with_policy(
        Authenticator::legacy(token),
        workspace,
        policy,
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    )
}

#[cfg(test)]
fn router_with_auth(auth: Authenticator, workspace: Workspace, policy: ApiPolicy) -> Router {
    router_with_policy(
        auth,
        workspace,
        policy,
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    )
}

#[cfg(test)]
fn router_with_deploy(
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
    deploy: DeployPolicy,
) -> Router {
    router_with_policy(auth, workspace, policy, deploy, None, BODY_LIMIT_BYTES)
}

/// Canonical `(method, path)` inventory for the HTTP management API.
///
/// Keep this list in lockstep with `router_with_policy`. Black-box E2E tests
/// parse the markers below so route drift fails the suite without importing
/// the binary crate as a library.
// OMAKURE_HTTP_ROUTE_INVENTORY_START
#[allow(dead_code)] // consumed by black-box E2E via source markers; kept as router source of truth
pub const HTTP_ROUTE_INVENTORY: &[(&str, &str)] = &[
    ("GET", "/v1/health"),
    ("GET", "/v1/ready"),
    ("GET", "/v1/admin/status"),
    ("GET", "/v1/config"),
    ("GET", "/v1/doctor"),
    ("GET", "/v1/workspace"),
    ("GET", "/v1/search"),
    ("GET", "/v1/tree"),
    ("GET", "/v1/tree/*path"),
    ("GET", "/v1/scripts"),
    ("GET", "/v1/scripts/*script_id"),
    ("GET", "/v1/envs"),
    ("POST", "/v1/envs"),
    ("DELETE", "/v1/envs/active"),
    ("GET", "/v1/envs/:name"),
    ("PUT", "/v1/envs/:name"),
    ("PATCH", "/v1/envs/:name"),
    ("DELETE", "/v1/envs/:name"),
    ("POST", "/v1/envs/:name/activate"),
    ("PUT", "/v1/envs/:name/params/:key"),
    ("DELETE", "/v1/envs/:name/params/:key"),
    ("GET", "/v1/runs"),
    ("POST", "/v1/runs"),
    ("GET", "/v1/runs/:run_id"),
    ("GET", "/v1/runs/:run_id/traces"),
    ("POST", "/v1/runs/:run_id/cancel"),
    ("POST", "/v1/runs/:run_id/dead-letter"),
    ("GET", "/v1/queue/stats"),
    ("GET", "/v1/batteries"),
    ("POST", "/v1/batteries"),
    ("GET", "/v1/batteries/:battery_id"),
    ("DELETE", "/v1/batteries/:battery_id"),
    ("GET", "/v1/batteries/:battery_id/scripts"),
    (
        "POST",
        "/v1/batteries/:battery_id/scripts/:script_id/install",
    ),
    ("POST", "/v1/batteries/:battery_id/sync"),
    ("GET", "/v1/secrets"),
    ("GET", "/v1/node/status"),
    ("POST", "/v1/node/init"),
    ("GET", "/v1/node/peers"),
    ("POST", "/v1/node/peers"),
    ("PATCH", "/v1/node/peers/:node_id/capabilities"),
    ("POST", "/v1/node/peers/:node_id/revoke"),
];
// OMAKURE_HTTP_ROUTE_INVENTORY_END

fn router_with_policy(
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
    deploy: DeployPolicy,
    readiness: Option<Arc<ReadinessGate>>,
    body_limit: usize,
) -> Router {
    let auth_verification_gate = Arc::new(tokio::sync::Semaphore::new(
        deploy
            .auth
            .max_concurrent_verifications
            .clamp(1, policy::MAX_CONCURRENT_AUTH_VERIFICATIONS),
    ));
    let state = ApiState {
        auth,
        workspace,
        policy,
        deploy,
        readiness,
        auth_verification_gate,
    };
    // Route registration must stay aligned with `HTTP_ROUTE_INVENTORY`.
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/ready", get(ready_handler))
        .route("/v1/admin/status", get(admin_status_handler))
        .route("/v1/config", get(config_handler))
        .route("/v1/doctor", get(doctor_handler))
        .route("/v1/workspace", get(workspace_handler))
        .route("/v1/search", get(search_handler))
        .route("/v1/tree", get(tree_root_handler))
        .route("/v1/tree/*path", get(tree_path_handler))
        .route("/v1/scripts", get(list_scripts_handler))
        .route("/v1/scripts/*script_id", get(script_path_handler))
        .route("/v1/envs", get(list_envs_handler).post(create_env_handler))
        .route("/v1/envs/active", delete(deactivate_env_handler))
        .route(
            "/v1/envs/:name",
            get(show_env_handler)
                .put(put_env_handler)
                .patch(patch_env_handler)
                .delete(delete_env_handler),
        )
        .route("/v1/envs/:name/activate", post(activate_env_handler))
        .route(
            "/v1/envs/:name/params/:key",
            put(set_env_param_handler).delete(delete_env_param_handler),
        )
        .route("/v1/runs", get(list_runs_handler).post(enqueue_run_handler))
        .route("/v1/runs/:run_id", get(show_run_handler))
        .route("/v1/runs/:run_id/traces", get(list_traces_handler))
        .route("/v1/runs/:run_id/cancel", post(cancel_run_handler))
        .route(
            "/v1/runs/:run_id/dead-letter",
            post(dead_letter_run_handler),
        )
        .route("/v1/queue/stats", get(queue_stats_handler))
        .route(
            "/v1/batteries",
            get(list_batteries_handler).post(add_battery_handler),
        )
        .route(
            "/v1/batteries/:battery_id",
            get(inspect_battery_handler).delete(remove_battery_handler),
        )
        .route(
            "/v1/batteries/:battery_id/scripts",
            get(list_battery_scripts_handler),
        )
        .route(
            "/v1/batteries/:battery_id/scripts/:script_id/install",
            post(install_battery_script_handler),
        )
        .route("/v1/batteries/:battery_id/sync", post(sync_battery_handler))
        .route("/v1/secrets", get(list_secrets_metadata_handler))
        .route("/v1/node/status", get(node_status_handler))
        .route("/v1/node/init", post(node_initialize_handler))
        .route(
            "/v1/node/peers",
            get(node_peers_handler).post(node_trust_handler),
        )
        .route(
            "/v1/node/peers/:node_id/capabilities",
            axum::routing::patch(node_capabilities_handler),
        )
        .route("/v1/node/peers/:node_id/revoke", post(node_revoke_handler))
        .fallback(protected_not_found)
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json::ok_envelope(HealthResponse { status: "ok" }))
}

async fn ready_handler(State(state): State<ApiState>) -> Response {
    let ready = state
        .readiness
        .as_ref()
        .map(|gate| gate.is_ready())
        .unwrap_or(true);
    if ready {
        (
            StatusCode::OK,
            Json(json::ok_envelope(ReadyResponse { status: "ready" })),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json::ok_envelope(ReadyResponse {
                status: "not_ready",
            })),
        )
            .into_response()
    }
}

async fn admin_status_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "admin:status") {
        return response;
    }
    let (ready, readiness) = match &state.readiness {
        Some(gate) => (
            gate.is_ready(),
            AdminReadinessDetails {
                requires_worker: gate.requires_worker,
                requires_scheduler: gate.requires_scheduler,
                workers_configured: gate.workers_configured,
                scheduler_configured: gate.scheduler_configured,
                workers_alive: gate.workers_alive.load(Ordering::SeqCst),
                scheduler_alive: gate.scheduler_alive.load(Ordering::SeqCst),
            },
        ),
        None => (
            true,
            AdminReadinessDetails {
                requires_worker: false,
                requires_scheduler: false,
                workers_configured: false,
                scheduler_configured: false,
                workers_alive: false,
                scheduler_alive: false,
            },
        ),
    };
    (
        StatusCode::OK,
        Json(json::ok_envelope(AdminStatusResponse {
            ready,
            readiness,
            auth: state.auth.status(),
        })),
    )
        .into_response()
}

async fn workspace_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ConfigRead) {
        return response;
    }
    operation_response(core::workspace_summary(&state.workspace))
}

async fn node_status_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::NodeRead) {
        return response;
    }
    operation_response(node_context().and_then(|context| node_ops::public_node_status(&context)))
}

async fn node_initialize_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::NodeWrite) {
        return response;
    }
    if let Err(error) =
        parse_json_body::<NodeInitializeBody>(body, state.deploy.http.body_limit_bytes).await
    {
        return operation_error_response(error);
    }
    operation_response(node_context().and_then(|context| {
        node_ops::initialize_node_nonblocking(&context, &crate::domain::NodeConfig::default())
    }))
}

async fn node_peers_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::NodeRead) {
        return response;
    }
    operation_response(node_context().and_then(|context| node_ops::list_trusted_peers(&context)))
}

async fn node_trust_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "trust:write") {
        return response;
    }
    let body =
        match parse_json_body::<ManualTrustBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::import_manual_trust(
            &context,
            node_ops::ManualTrustRequest {
                node_id: body.node_id,
                public_key: body.public_key,
                role: body.role,
                capabilities: body.capabilities,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
            },
        )
    }))
}

async fn node_capabilities_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(node_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "trust:write") {
        return response;
    }
    let body =
        match parse_json_body::<NodeCapabilitiesBody>(body, state.deploy.http.body_limit_bytes)
            .await
        {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::update_peer_capabilities(
            &context,
            node_ops::CapabilityUpdateRequest {
                node_id,
                capabilities: body.capabilities,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
            },
        )
    }))
}

async fn node_revoke_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(node_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "trust:write") {
        return response;
    }
    let body =
        match parse_json_body::<NodeRevokeBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::revoke_peer(
            &context,
            node_ops::RevocationRequest {
                node_id,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
            },
        )
    }))
}

fn node_context() -> OperationResult<crate::node::NodeContext> {
    node_ops::resolve_context(crate::node::NodePathOverrides::default())
}

async fn config_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ConfigRead) {
        return response;
    }
    operation_response(config_ops::redacted_config_summary(&state.workspace))
}

async fn doctor_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ConfigRead) {
        return response;
    }
    operation_response(doctor_ops::doctor_report(&state.workspace))
}

async fn search_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    let request = query_pairs(raw_query.as_deref()).and_then(|pairs| {
        let query = query_value(&pairs, "q")
            .or_else(|| query_value(&pairs, "query"))
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    "q query parameter is required",
                )
            })?;
        if query.trim().is_empty() {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                "q query parameter must not be empty",
            ));
        }
        if query.len() > MAX_SEARCH_QUERY_LEN {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                "q query parameter is too long",
            ));
        }
        let tags = query_values(&pairs, "tag");
        if tags.len() > MAX_SEARCH_TAGS || tags.iter().any(|tag| tag.len() > MAX_SEARCH_TAG_LEN) {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                "tag query parameters exceed limits",
            ));
        }
        Ok(search_ops::SearchScriptsRequest {
            query,
            tags,
            refresh: false,
        })
    });
    operation_response(
        request.and_then(|request| search_ops::search_scripts(&state.workspace, request)),
    )
}

async fn list_scripts_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    let request = query_pairs(raw_query.as_deref()).map(|pairs| core::ListScriptsRequest {
        tags: query_values(&pairs, "tag"),
    });
    operation_response(request.and_then(|request| core::list_scripts(&state.workspace, request)))
}

async fn describe_script_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    script_id: String,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(core::describe_script(
        &state.workspace,
        core::DescribeScriptRequest { script: script_id },
    ))
}

async fn script_schema_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    script_id: String,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(
        core::describe_script(
            &state.workspace,
            core::DescribeScriptRequest { script: script_id },
        )
        .map(|description| description.schema),
    )
}

async fn script_path_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(script_id): AxumPath<String>,
) -> Response {
    if let Some(script_id) = script_id.strip_suffix("/content") {
        if !script_id.is_empty() {
            return script_content_handler(
                State(state),
                Extension(auth_ctx),
                script_id.to_string(),
            )
            .await;
        }
    }
    match script_id.strip_suffix("/schema") {
        Some(script_id) if !script_id.is_empty() => {
            script_schema_handler(State(state), Extension(auth_ctx), script_id.to_string()).await
        }
        _ => describe_script_handler(State(state), Extension(auth_ctx), script_id).await,
    }
}

async fn script_content_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    script_id: String,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::read_script_content_limited(
        &state.workspace,
        scripts_ops::ReadScriptContentRequest { script: script_id },
        state.deploy.scripts.max_content_bytes as u64,
    ))
}

async fn tree_root_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::list_tree_limited(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: None },
        state.deploy.scripts.tree_entry_limit,
    ))
}

async fn tree_path_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::list_tree_limited(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: Some(path) },
        state.deploy.scripts.tree_entry_limit,
    ))
}

async fn list_envs_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvRead) {
        return response;
    }
    operation_response(env_ops::list_envs(&state.workspace))
}

fn env_params_forbid_secret_refs(
    deploy: &crate::policy::DeployPolicy,
    params: &[env_ops::EnvParam],
) -> Option<Response> {
    if deploy.envs.allow_secret_refs {
        return None;
    }
    if params.iter().any(|p| p.value.starts_with("secret://")) {
        return Some(operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.allow_secret_refs=false",
        )));
    }
    None
}

fn env_value_forbid_secret_ref(
    deploy: &crate::policy::DeployPolicy,
    value: &str,
) -> Option<Response> {
    if deploy.envs.allow_secret_refs || !value.starts_with("secret://") {
        return None;
    }
    Some(operation_error_response(OperationError::new(
        OperationErrorCode::Forbidden,
        "policy envs.allow_secret_refs=false",
    )))
}

async fn create_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvBody>(body, state.deploy.http.body_limit_bytes).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    let Some(name) = body.name else {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "name is required",
        ));
    };
    if let Some(response) = env_params_forbid_secret_refs(&state.deploy, &body.params) {
        return response;
    }
    operation_response(env_ops::create_env(&state.workspace, &name, &body.params))
}

async fn show_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvRead) {
        return response;
    }
    operation_response(env_ops::show_env(&state.workspace, &name))
}

async fn put_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvBody>(body, state.deploy.http.body_limit_bytes).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if let Some(response) = env_params_forbid_secret_refs(&state.deploy, &body.params) {
        return response;
    }
    let result = match env_ops::replace_env(&state.workspace, &name, &body.params) {
        Err(err) if err.code == OperationErrorCode::NotFound => {
            env_ops::create_env(&state.workspace, &name, &body.params)
        }
        other => other,
    };
    operation_response(result)
}

async fn patch_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvBody>(body, state.deploy.http.body_limit_bytes).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if let Some(response) = env_params_forbid_secret_refs(&state.deploy, &body.params) {
        return response;
    }
    for param in body.params {
        if let Err(err) = env_ops::set_param(&state.workspace, &name, &param.key, &param.value) {
            return operation_error_response(err);
        }
    }
    operation_response(Ok(()))
}

async fn delete_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    operation_response(env_ops::delete_env(&state.workspace, &name))
}

async fn set_env_param_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath((name, key)): AxumPath<(String, String)>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvParamBody>(body, state.deploy.http.body_limit_bytes).await
    {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if let Some(response) = env_value_forbid_secret_ref(&state.deploy, &body.value) {
        return response;
    }
    operation_response(env_ops::set_param(
        &state.workspace,
        &name,
        &key,
        &body.value,
    ))
}

async fn delete_env_param_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath((name, key)): AxumPath<(String, String)>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    operation_response(env_ops::remove_param(&state.workspace, &name, &key))
}

async fn activate_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvActivate) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    operation_response(env_ops::activate_env(&state.workspace, &name))
}

async fn deactivate_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvActivate) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    operation_response(env_ops::deactivate_env(&state.workspace))
}

async fn list_runs_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    let request = list_runs_request(raw_query.as_deref());
    operation_response(request.and_then(|request| core::list_runs(&state.workspace, request)))
}

async fn show_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    operation_response(core::show_run(
        &state.workspace,
        core::ShowRunRequest { run_id },
    ))
}

async fn list_traces_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    let request = list_traces_request(run_id, raw_query.as_deref());
    operation_response(request.and_then(|request| core::list_traces(&state.workspace, request)))
}

async fn queue_stats_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    operation_response(core::queue_stats(&state.workspace))
}

async fn enqueue_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "runs:enqueue") {
        return response;
    }
    let body =
        match parse_json_body::<EnqueueRunBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let requested_run_id = body.run_id.as_deref().and_then(safe_audit_run_id);
    if body.env.is_some() {
        if !state.deploy.runs.allow_env_selection {
            return attach_audit_run_id(
                operation_error_response(OperationError::new(
                    OperationErrorCode::Forbidden,
                    "policy runs.allow_env_selection=false",
                )),
                requested_run_id,
            );
        }
        if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::EnvUse) {
            return attach_audit_run_id(response, requested_run_id);
        }
    }
    let args_use_secret_provider = args_use_secret_provider(&body.args);
    if !state.deploy.runs.allow_secret_fields
        && (!body.secret_fields.is_empty() || args_use_secret_provider)
    {
        return attach_audit_run_id(
            operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                "policy runs.allow_secret_fields=false",
            )),
            requested_run_id,
        );
    }
    if !body.secret_fields.is_empty() || args_use_secret_provider {
        if let Some(response) =
            require_capability(&state, &auth_ctx, ApiCapability::SecretProviderUse)
        {
            return attach_audit_run_id(response, requested_run_id);
        }
    }
    if let Some(response) = require_implicit_secret_capabilities(&state, &auth_ctx, &body) {
        return attach_audit_run_id(response, requested_run_id);
    }
    if body
        .secret_fields
        .values()
        .any(|value| !value.starts_with("secret://"))
    {
        return attach_audit_run_id(
            operation_error_response(OperationError::new(
                OperationErrorCode::InvalidInput,
                "queued HTTP secret_fields must use secret:// refs so workers can resolve them without persisting plaintext",
            )),
            requested_run_id,
        );
    }
    let result = core::enqueue_run_with_access(
        &state.workspace,
        core::EnqueueRunRequest {
            script: body.script,
            args: body.args,
            env: body.env,
            secret_fields: body.secret_fields.into_iter().collect(),
            run_id: body.run_id,
            actor: body.actor,
            reason: body.reason,
            priority: body.priority,
            timeout_ms: body.timeout_ms,
            parent_run_id: body.parent_run_id,
            cron_schedule_id: body.cron_schedule_id,
        },
        &state
            .policy
            .secret_access(&auth_ctx, state.auth.is_file_mode()),
    );
    let run_id = result
        .as_ref()
        .ok()
        .map(|row| row.run_id.clone())
        .or(requested_run_id);
    operation_response_with_run_id(result, run_id)
}

fn require_implicit_secret_capabilities(
    state: &ApiState,
    auth_ctx: &AuthContext,
    body: &EnqueueRunBody,
) -> Option<Response> {
    let description = match core::describe_script(
        &state.workspace,
        core::DescribeScriptRequest {
            script: body.script.clone(),
        },
    ) {
        Ok(description) => description,
        Err(err) => return Some(operation_error_response(err)),
    };
    let repo = crate::adapters::workspace_repository::FsWorkspaceRepository::new(
        state.workspace.scripts_root().to_path_buf(),
    );
    let schema = match repo.read_schema(std::path::Path::new(&description.absolute_path)) {
        Ok(schema) => schema,
        Err(err) => {
            return Some(operation_error_response(OperationError::new(
                OperationErrorCode::InvalidInput,
                err.to_string(),
            )))
        }
    };
    let secret_fields: Vec<_> = schema
        .fields
        .iter()
        .filter(|field| field.is_secret())
        .collect();
    if secret_fields.is_empty() {
        return None;
    }

    if secret_fields.iter().any(|field| field.default.is_some()) {
        if !state.deploy.runs.allow_secret_fields {
            return Some(operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                "policy runs.allow_secret_fields=false",
            )));
        }
        if let Some(response) =
            require_capability(state, auth_ctx, ApiCapability::SecretProviderUse)
        {
            return Some(response);
        }
    }

    let env_file = match body
        .env
        .as_deref()
        .map(|name| crate::operations::envs::env_file_path(&state.workspace, name))
        .transpose()
    {
        Ok(path) => path,
        Err(err) => return Some(operation_error_response(err)),
    };
    let run_env = match crate::adapters::environments::resolve_run_env(
        state.workspace.envs_dir(),
        env_file.as_deref(),
    ) {
        Ok(run_env) => run_env,
        Err(err) => {
            return Some(operation_error_response(OperationError::new(
                OperationErrorCode::InvalidInput,
                err.to_string(),
            )))
        }
    };
    if secret_fields.iter().any(|field| {
        run_env
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(&field.name))
    }) {
        if let Some(response) = require_capability(state, auth_ctx, ApiCapability::EnvUse) {
            return Some(response);
        }
    }

    None
}

fn args_use_secret_provider(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg.starts_with("secret://")
            || arg
                .split_once('=')
                .map(|(_, value)| value.starts_with("secret://"))
                .unwrap_or(false)
    })
}

async fn cancel_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "runs:cancel") {
        return response;
    }
    let body =
        match parse_json_body::<RunReasonBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let result = core::cancel_run(
        &state.workspace,
        core::CancelRunRequest {
            run_id: run_id.clone(),
            reason: body.reason,
        },
    );
    operation_response_with_run_id(result, Some(run_id))
}

async fn dead_letter_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "runs:dead-letter") {
        return response;
    }
    let body =
        match parse_json_body::<RunReasonBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let result = core::dead_letter_run(
        &state.workspace,
        core::DeadLetterRunRequest {
            run_id: run_id.clone(),
            reason: body.reason,
        },
    );
    operation_response_with_run_id(result, Some(run_id))
}

async fn list_secrets_metadata_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if !state.deploy.secrets.metadata_endpoint {
        return error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "secrets metadata endpoint is disabled by policy",
        );
    }
    if let Some(response) =
        require_capability(&state, &auth_ctx, ApiCapability::SecretsReadMetadata)
    {
        return response;
    }
    let access = state
        .policy
        .secret_access(&auth_ctx, state.auth.is_file_mode());
    // Metadata listing also accepts credentials:use as a read-adjacent scope when
    // secrets:read-metadata is granted; secret_access already folds both scopes.
    let metadata = crate::secrets::list_secret_metadata(&state.workspace, &access);
    (StatusCode::OK, Json(json::ok_envelope(metadata))).into_response()
}

async fn list_batteries_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::BatteryRead) {
        return response;
    }
    operation_response(battery_ops::list_batteries(&state.workspace))
}

async fn add_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "batteries:add") {
        return response;
    }
    let body =
        match parse_json_body::<AddBatteryBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    if !body
        .git_url
        .trim()
        .to_ascii_lowercase()
        .starts_with("https://")
    {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "HTTP API battery registration only accepts https git URLs",
        ));
    }
    if let Err(err) = battery_ops::assert_local_battery_allowed(
        state.deploy.sources.allow_local_batteries,
        &body.git_url,
    ) {
        return operation_error_response(err);
    }
    if !state.deploy.sources.allow_https_batteries {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy sources.allow_https_batteries=false",
        ));
    }
    // SSRF guard (registration): reject literal private/loopback/metadata hosts
    // up front. The resolving check runs at sync time before the actual fetch.
    if let Err(err) = battery_ops::assert_git_url_host_public_literal(&body.git_url) {
        return operation_error_response(err);
    }
    if body
        .token_ref
        .as_ref()
        .is_some_and(|r| !r.trim().is_empty())
    {
        if !state.deploy.sources.allow_private_https_batteries {
            return operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                "policy sources.allow_private_https_batteries=false",
            ));
        }
        if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::CredentialsUse)
        {
            return response;
        }
        // Validate ACL only — secret need not exist until sync.
        let access = state
            .policy
            .battery_credential_access(&auth_ctx, state.auth.is_file_mode());
        if let Err(err) =
            crate::secrets::check_secret_access(body.token_ref.as_deref().unwrap_or(""), &access)
        {
            return operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                format!("token_ref not usable: {err}"),
            ));
        }
    }
    operation_response(battery_ops::add_battery(
        &state.workspace,
        battery_ops::AddBatteryRequest {
            name: body.name,
            git_url: body.git_url,
            requested_ref: body.requested_ref,
            token_ref: body.token_ref,
        },
    ))
}

async fn sync_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "batteries:sync") {
        return response;
    }
    // Cheap authz/validation stays on the reactor; it must run BEFORE the
    // heavy work so an authz failure returns 403 without any network I/O.
    let prepared = (|| -> OperationResult<_> {
        require_https_battery_source(&state.workspace, &battery_id)?;
        let batteries = battery_ops::list_batteries(&state.workspace)?;
        let summary = batteries
            .into_iter()
            .find(|b| b.name == battery_id)
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::NotFound,
                    format!("battery '{battery_id}' was not found"),
                )
            })?;
        if summary.auth.is_some() {
            if !state.deploy.sources.allow_private_https_batteries {
                return Err(OperationError::new(
                    OperationErrorCode::Forbidden,
                    "policy sources.allow_private_https_batteries=false",
                ));
            }
            let file_mode = state.auth.is_file_mode();
            let has_credentials = if file_mode {
                auth_ctx.has_scope("credentials:use") || auth_ctx.has_scope("*")
            } else {
                state.policy.permits(ApiCapability::CredentialsUse)
            };
            if !has_credentials {
                return Err(OperationError::new(
                    OperationErrorCode::Forbidden,
                    "credentials:use scope is required for private HTTPS Battery sync",
                ));
            }
        }
        let access = state
            .policy
            .battery_credential_access(&auth_ctx, state.auth.is_file_mode());
        Ok(access)
    })();

    let result = match prepared {
        Err(err) => Err(err),
        Ok(access) => {
            // DNS resolution (SSRF guard) + git subprocess are blocking; run them
            // off the async runtime so a slow/large sync never stalls the reactor.
            // `sync_battery_https_only_with_access` resolves the stored host and
            // refuses private/loopback/link-local/metadata targets before
            // issuing any git command (see `resolve_public_git_endpoint`) — no
            // separate pre-check needed here.
            let workspace = state.workspace.clone_for_executor();
            let name = battery_id;
            let join = tokio::task::spawn_blocking(move || -> OperationResult<_> {
                battery_ops::sync_battery_https_only_with_access(
                    &workspace,
                    battery_ops::SyncBatteryRequest { name },
                    &access,
                )
            })
            .await;
            match join {
                Ok(inner) => inner,
                Err(_) => Err(OperationError::new(
                    OperationErrorCode::IoFailed,
                    "battery sync task failed to complete",
                )),
            }
        }
    };
    operation_response(result)
}

async fn inspect_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::BatteryRead) {
        return response;
    }
    let request = require_https_battery_source(&state.workspace, &battery_id)
        .map(|_| battery_ops::InspectBatteryRequest { name: battery_id });
    operation_response(
        request.and_then(|request| battery_ops::inspect_battery(&state.workspace, request)),
    )
}

async fn list_battery_scripts_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, &auth_ctx, ApiCapability::BatteryRead) {
        return response;
    }
    let request = require_https_battery_source(&state.workspace, &battery_id)
        .map(|_| battery_ops::InspectBatteryRequest { name: battery_id });
    operation_response(
        request.and_then(|request| battery_ops::list_battery_scripts(&state.workspace, request)),
    )
}

async fn install_battery_script_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath((battery_id, script_id)): AxumPath<(String, String)>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "batteries:install") {
        return response;
    }
    let body =
        match parse_json_body::<InstallBatteryScriptBody>(body, state.deploy.http.body_limit_bytes)
            .await
        {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let request = require_https_battery_source(&state.workspace, &battery_id).map(|_| {
        battery_ops::InstallBatteryScriptRequest {
            battery_name: battery_id,
            script_id,
            force: body.force,
        }
    });
    operation_response(
        request.and_then(|request| battery_ops::install_battery_script(&state.workspace, request)),
    )
}

async fn remove_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_scope(&state, &auth_ctx, "batteries:remove") {
        return response;
    }
    let request = query_pairs(raw_query.as_deref()).and_then(|pairs| {
        query_bool(&pairs, "remove_cache").map(|remove_cache| battery_ops::RemoveBatteryRequest {
            name: battery_id,
            remove_cache: remove_cache.unwrap_or(false),
        })
    });
    operation_response(
        request.and_then(|request| battery_ops::remove_battery(&state.workspace, request)),
    )
}

async fn protected_not_found() -> impl IntoResponse {
    error_response(StatusCode::NOT_FOUND, "not_found", "endpoint not found")
}

// Concurrent Argon2id verifications are capped by
// `deploy.auth.max_concurrent_verifications` (see
// `policy::DEFAULT_MAX_CONCURRENT_AUTH_VERIFICATIONS`). Each verify is
// memory-hard (~64 MiB); without a bound, unauthenticated requests carrying
// any bearer string could amplify hashing into CPU/memory exhaustion.
// Verifies also run on the blocking pool (see `require_bearer`) so async
// reactor threads never stall — keeping `/v1/health` and `/v1/ready`
// responsive even under an auth flood.

/// Authenticate a presented bearer token without blocking the async runtime.
/// The memory-hard verify runs on the blocking pool under a concurrency permit.
enum AuthAttempt {
    Accepted(AuthContext),
    Rejected,
    Busy,
}

async fn authenticate_off_runtime(
    auth: &Authenticator,
    gate: &Arc<tokio::sync::Semaphore>,
    presented: &str,
) -> AuthAttempt {
    // Hold the permit for the LIFETIME OF THE HASH, not of the request future.
    // `spawn_blocking` is detached: if the client cancels mid-verify the request
    // future is dropped, but the Argon2 task keeps running. Moving an *owned*
    // permit into the blocking closure ties the permit's release to the hash
    // completing, so a "send bearer then reset connection" flood cannot orphan
    // unbounded memory-hard hashes past MAX_CONCURRENT_AUTH_VERIFICATIONS.
    let Ok(permit) = Arc::clone(gate).try_acquire_owned() else {
        return AuthAttempt::Busy;
    };
    let auth = auth.clone();
    let token = presented.to_string();
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth.authenticate(&token)
    })
    .await
    {
        Ok(Some(context)) => AuthAttempt::Accepted(context),
        Ok(None) | Err(_) => AuthAttempt::Rejected,
    }
}

async fn require_bearer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().as_str().to_string();
    if path == "/v1/health" || path == "/v1/ready" {
        return next.run(request).await;
    }

    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    let authenticated = match presented {
        Some(token) => {
            authenticate_off_runtime(&state.auth, &state.auth_verification_gate, token).await
        }
        None => AuthAttempt::Rejected,
    };

    match authenticated {
        AuthAttempt::Accepted(ctx) => {
            // Deploy policy gates route groups before token scopes.
            if let Some(message) = state.deploy.deny_reason(&method, &path) {
                emit_http_audit(HttpAuditEvent {
                    token_id: Some(ctx.token_id),
                    run_id: mutation_path_run_id(&method, &path),
                    method,
                    path,
                    outcome: "forbidden".to_string(),
                    status: StatusCode::FORBIDDEN.as_u16(),
                });
                return error_response(StatusCode::FORBIDDEN, "forbidden", message);
            }
            let token_id = ctx.token_id.clone();
            request.extensions_mut().insert(ctx);
            let response = next.run(request).await;
            let status = response.status().as_u16();
            let run_id = response
                .extensions()
                .get::<AuditRunId>()
                .and_then(|value| safe_audit_run_id(&value.0))
                .or_else(|| mutation_path_run_id(&method, &path));
            let outcome = if (200..400).contains(&status) {
                "ok"
            } else if status == 403 {
                "forbidden"
            } else if status == 401 {
                "unauthorized"
            } else {
                "error"
            };
            emit_http_audit(HttpAuditEvent {
                token_id: Some(token_id),
                run_id,
                method,
                path,
                outcome: outcome.to_string(),
                status,
            });
            response
        }
        AuthAttempt::Rejected => {
            emit_http_audit(HttpAuditEvent {
                token_id: None,
                run_id: None,
                method,
                path,
                outcome: "unauthorized".to_string(),
                status: StatusCode::UNAUTHORIZED.as_u16(),
            });
            error_response(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "bearer token required",
            )
        }
        AuthAttempt::Busy => {
            emit_http_audit(HttpAuditEvent {
                token_id: None,
                run_id: None,
                method,
                path,
                outcome: "unavailable".to_string(),
                status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            });
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_busy",
                "authentication capacity is temporarily exhausted",
            )
        }
    }
}

fn safe_audit_run_id(run_id: &str) -> Option<String> {
    (!run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| run_id.to_string())
}

fn mutation_path_run_id(method: &str, path: &str) -> Option<String> {
    if method != "POST" {
        return None;
    }
    let suffix = path.strip_prefix("/v1/runs/")?;
    let (run_id, mutation) = suffix.split_once('/')?;
    matches!(mutation, "cancel" | "dead-letter")
        .then(|| safe_audit_run_id(run_id))
        .flatten()
}

async fn parse_json_body<T: for<'de> Deserialize<'de>>(
    body: Body,
    limit_bytes: usize,
) -> OperationResult<T> {
    let bytes = to_bytes(body, limit_bytes).await.map_err(|err| {
        let message = err.to_string();
        if message.contains("length limit") {
            OperationError::new(
                OperationErrorCode::PayloadTooLarge,
                "request body is too large",
            )
        } else {
            OperationError::new(
                OperationErrorCode::InvalidInput,
                format!("invalid request body: {message}"),
            )
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|err| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("invalid JSON request body: {err}"),
        )
    })
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json::err_envelope(code, message))).into_response()
}

fn require_capability(
    state: &ApiState,
    auth: &AuthContext,
    capability: ApiCapability,
) -> Option<Response> {
    require_scope(state, auth, capability.as_scope())
}

fn require_scope(state: &ApiState, auth: &AuthContext, scope: &str) -> Option<Response> {
    let allowed = if state.auth.is_file_mode() {
        auth.has_scope(scope)
    } else {
        // Legacy mode: map plan scopes back to process-wide capabilities.
        // `admin:status` requires an explicit `--capability admin:status` (or `all`).
        let capability = match scope {
            "config:read" | "doctor:read" | "workspace:read" => ApiCapability::ConfigRead,
            "scripts:read" | "search:read" => ApiCapability::ScriptsRead,
            "envs:read" | "env:read" => ApiCapability::EnvRead,
            "envs:write" | "env:write" => ApiCapability::EnvWrite,
            "envs:activate" | "env:activate" => ApiCapability::EnvActivate,
            "envs:use" | "env:use" => ApiCapability::EnvUse,
            "secrets:use" => ApiCapability::SecretProviderUse,
            "secrets:read-metadata" => ApiCapability::SecretsReadMetadata,
            "credentials:use" => ApiCapability::CredentialsUse,
            "runs:read" => ApiCapability::RunRead,
            "runs:write" | "runs:enqueue" | "runs:cancel" | "runs:dead-letter" => {
                ApiCapability::RunWrite
            }
            "batteries:read" => ApiCapability::BatteryRead,
            "batteries:write" | "batteries:add" | "batteries:sync" | "batteries:install"
            | "batteries:remove" => ApiCapability::BatteryWrite,
            "admin:status" => ApiCapability::AdminStatus,
            "node:read" => ApiCapability::NodeRead,
            "node:write" => ApiCapability::NodeWrite,
            "trust:write" => ApiCapability::TrustWrite,
            _ => {
                return Some(error_response(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "token is not permitted for this operation",
                ))
            }
        };
        state.policy.permits(capability)
    };
    (!allowed).then(|| {
        error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            "token is not permitted for this operation",
        )
    })
}

fn operation_response<T: Serialize>(result: OperationResult<T>) -> Response {
    match result {
        Ok(data) => (StatusCode::OK, Json(json::ok_envelope(data))).into_response(),
        Err(err) => operation_error_response(err),
    }
}

fn operation_response_with_run_id<T: Serialize>(
    result: OperationResult<T>,
    run_id: Option<String>,
) -> Response {
    attach_audit_run_id(operation_response(result), run_id)
}

fn attach_audit_run_id(mut response: Response, run_id: Option<String>) -> Response {
    if let Some(run_id) = run_id {
        response.extensions_mut().insert(AuditRunId(run_id));
    }
    response
}

fn operation_error_response(err: OperationError) -> Response {
    let status = match err.code {
        OperationErrorCode::InvalidInput
        | OperationErrorCode::UnsafePath
        | OperationErrorCode::ManifestInvalid => StatusCode::BAD_REQUEST,
        OperationErrorCode::Forbidden => StatusCode::FORBIDDEN,
        OperationErrorCode::NotFound => StatusCode::NOT_FOUND,
        OperationErrorCode::AlreadyExists
        | OperationErrorCode::Conflict
        | OperationErrorCode::NotSynced => StatusCode::CONFLICT,
        OperationErrorCode::UnsupportedScript => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        OperationErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        OperationErrorCode::GitFailed
        | OperationErrorCode::IoFailed
        | OperationErrorCode::RegistryInvalid => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, err.code.as_str(), &err.message)
}

fn require_https_battery_source(workspace: &Workspace, battery_id: &str) -> OperationResult<()> {
    let battery = battery_ops::list_batteries(workspace)?
        .into_iter()
        .find(|battery| battery.name == battery_id)
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotFound,
                format!("battery '{battery_id}' was not found"),
            )
        })?;
    if battery
        .git_url
        .trim()
        .to_ascii_lowercase()
        .starts_with("https://")
    {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "HTTP API can only operate on Batteries registered with https git URLs",
        ))
    }
}

fn list_runs_request(raw_query: Option<&str>) -> OperationResult<core::ListRunsRequest> {
    let pairs = query_pairs(raw_query)?;
    Ok(core::ListRunsRequest {
        script: query_value(&pairs, "script"),
        actor: query_value(&pairs, "actor"),
        since_ms: query_i64(&pairs, "since_ms")?,
        until_ms: query_i64(&pairs, "until_ms")?,
        success: query_bool(&pairs, "success")?,
        limit: query_i64(&pairs, "limit")?,
        states: query_values(&pairs, "state"),
        state_set: query_value(&pairs, "state_set"),
    })
}

fn list_traces_request(
    run_id: String,
    raw_query: Option<&str>,
) -> OperationResult<core::ListTracesRequest> {
    let pairs = query_pairs(raw_query)?;
    Ok(core::ListTracesRequest {
        run_id,
        level: query_value(&pairs, "level"),
        since_sequence: query_i64(&pairs, "since_sequence")?,
    })
}

fn query_pairs(raw_query: Option<&str>) -> OperationResult<Vec<(String, String)>> {
    match raw_query {
        Some(query) => serde_urlencoded::from_str(query).map_err(|err| {
            OperationError::new(
                OperationErrorCode::InvalidInput,
                format!("invalid query string: {err}"),
            )
        }),
        None => Ok(Vec::new()),
    }
}

fn query_value(pairs: &[(String, String)], key: &str) -> Option<String> {
    query_values(pairs, key).into_iter().next()
}

fn query_values(pairs: &[(String, String)], key: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.clone())
        .collect()
}

fn query_i64(pairs: &[(String, String)], key: &str) -> OperationResult<Option<i64>> {
    query_value(pairs, key)
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    format!("invalid integer query parameter: {key}"),
                )
            })
        })
        .transpose()
}

fn query_bool(pairs: &[(String, String)], key: &str) -> OperationResult<Option<bool>> {
    query_value(pairs, key)
        .map(|value| {
            value.parse::<bool>().map_err(|_| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    format!("invalid boolean query parameter: {key}"),
                )
            })
        })
        .transpose()
}

#[allow(dead_code)] // retained for tests that build ApiPolicy without boot
pub(crate) fn policy_from_config(
    capabilities: &[String],
    refs: &[String],
) -> Result<ApiPolicy, Box<dyn Error>> {
    Ok(ApiPolicy::from_config(capabilities, refs)?)
}

#[allow(dead_code)] // thin wrapper; prefer resolve_auth_with_policy
pub(crate) fn resolve_auth(tokens_file: Option<&Path>) -> Result<Authenticator, ApiConfigError> {
    resolve_auth_with_policy(tokens_file, true)
}

pub(crate) fn resolve_auth_with_policy(
    tokens_file: Option<&Path>,
    allow_legacy_env_token: bool,
) -> Result<Authenticator, ApiConfigError> {
    let env_path = std::env::var("OMAKURE_TOKENS_FILE").ok();
    auth::resolve_authenticator_with_legacy(
        tokens_file,
        env_path.as_deref(),
        allow_legacy_env_token,
    )
    .map_err(|err| match err {
        auth::AuthError::MissingAuth => ApiConfigError::MissingToken,
        auth::AuthError::InvalidLegacyToken => ApiConfigError::InvalidToken,
        auth::AuthError::LegacyEnvTokenDisabled => {
            ApiConfigError::Auth(auth::AuthError::LegacyEnvTokenDisabled.to_string())
        }
        other => ApiConfigError::Auth(other.to_string()),
    })
}

pub(crate) fn validate_bind(
    addr: SocketAddr,
    allow_non_loopback: bool,
) -> Result<(), ApiConfigError> {
    if allow_non_loopback || addr.ip().is_loopback() {
        return Ok(());
    }

    Err(ApiConfigError::NonLoopbackBind(addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_meta;
    use crate::runs::{self, EnqueueOptions, RunCompletion};
    use axum::body::to_bytes;
    use axum::http::Method;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// Test sink for HTTP audit events (installs process-wide hook).
    /// Serialized so parallel tokio tests do not clobber the global hook.
    struct AuditCapture {
        events: Arc<Mutex<Vec<HttpAuditEvent>>>,
        _permit: tokio::sync::OwnedMutexGuard<()>,
    }

    impl AuditCapture {
        async fn install() -> Self {
            static AUDIT_TEST_LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
            let permit = AUDIT_TEST_LOCK
                .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
                .lock_owned()
                .await;
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&events);
            install_audit_hook(Arc::new(move |event: &HttpAuditEvent| {
                sink.lock().expect("audit lock").push(event.clone());
            }));
            Self {
                events,
                _permit: permit,
            }
        }

        fn events(&self) -> Vec<HttpAuditEvent> {
            self.events.lock().expect("audit lock").clone()
        }
    }

    impl Drop for AuditCapture {
        fn drop(&mut self) {
            clear_audit_hook();
        }
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), BODY_LIMIT_BYTES)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn workspace_in(dir: &TempDir) -> Workspace {
        let workspace = Workspace::new(dir.path().to_path_buf());
        workspace.ensure_layout().unwrap();
        workspace
    }

    fn write_script(root: &std::path::Path, name: &str) {
        if let Some(parent) = root.join(name).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            root.join(name),
            format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {{\"Name\":\"{name}\",\"Description\":\"test script\",\"Tags\":[\"ops\"],\"Fields\":[]}}\n# OMAKURE_SCHEMA_END\necho ok\n"
            ),
        )
        .unwrap();
    }

    fn write_secret_script(root: &std::path::Path, name: &str, default: Option<&str>) {
        let default_line = default
            .map(|value| format!(r#", "Default":"{value}""#))
            .unwrap_or_default();
        std::fs::write(
            root.join(name),
            format!(
                r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {{"Name":"Secret","Fields":[{{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"{default_line}}}]}}
# OMAKURE_SCHEMA_END
echo ok
"#
            ),
        )
        .unwrap();
    }

    fn authed_request(uri: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap()
    }

    fn authed_json_request(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn authed_json_method_request(method: Method, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn authed_delete_request(uri: &str) -> Request<Body> {
        Request::builder()
            .method(Method::DELETE)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap()
    }

    fn run_git(args: &[&str], cwd: &std::path::Path) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn invalid_manifest_repo() -> TempDir {
        let repo = TempDir::new().unwrap();
        run_git(&["init", "-b", "main"], repo.path());
        std::fs::write(repo.path().join("omakure-battery.toml"), "not = [valid").unwrap();
        run_git(&["add", "."], repo.path());
        run_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "invalid manifest",
            ],
            repo.path(),
        );
        repo
    }

    fn register_invalid_https_battery_cache(workspace: &Workspace, name: &str) {
        let paths = battery_ops::BatteryPaths::for_workspace(workspace);
        let cache = paths.cache_path_for(name);
        std::fs::create_dir_all(&cache).unwrap();
        run_git(&["init", "-b", "main"], &cache);
        std::fs::write(cache.join("omakure-battery.toml"), "not = [valid").unwrap();
        run_git(&["add", "."], &cache);
        run_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "invalid manifest",
            ],
            &cache,
        );
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&cache)
            .output()
            .unwrap();
        assert!(head.status.success());
        let registry = battery_ops::BatteryRegistry {
            version: battery_ops::REGISTRY_VERSION,
            batteries: vec![battery_ops::BatterySummary {
                name: name.to_string(),
                git_url: format!("https://example.invalid/{name}.git"),
                requested_ref: "main".into(),
                resolved_commit: Some(String::from_utf8_lossy(&head.stdout).trim().to_string()),
                cache_path: paths
                    .cache_path_for(name)
                    .strip_prefix(workspace.root())
                    .unwrap()
                    .to_path_buf(),
                last_synced_at: Some("2026-07-07T00:00:00Z".into()),
                auth: None,
            }],
        };
        std::fs::write(
            &paths.registry_path,
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn http_route_inventory_is_non_empty_and_unique() {
        assert!(!HTTP_ROUTE_INVENTORY.is_empty());
        let mut seen = std::collections::BTreeSet::new();
        for entry in HTTP_ROUTE_INVENTORY {
            assert!(
                seen.insert(*entry),
                "duplicate HTTP route inventory entry: {entry:?}"
            );
        }
    }

    #[test]
    fn http_route_inventory_matches_router_with_policy_registrations() {
        let source = include_str!("api.rs");
        let from_router = parse_router_route_registrations(source);
        let inventory: Vec<_> = HTTP_ROUTE_INVENTORY.to_vec();
        assert_eq!(
            from_router, inventory,
            "router_with_policy `.route(...)` registrations must equal HTTP_ROUTE_INVENTORY"
        );
    }

    fn parse_router_route_registrations(source: &str) -> Vec<(&str, &str)> {
        let start = source
            .find("fn router_with_policy(")
            .expect("router_with_policy");
        let after = &source[start..];
        let router_start = after.find("Router::new()").expect("Router::new");
        let block = &after[router_start..];
        // End at `.fallback(` which always follows the last `.route(...)`.
        let end = block.find(".fallback(").expect(".fallback after routes");
        let block = &block[..end];
        let mut routes = Vec::new();
        let mut i = 0;
        let bytes = block.as_bytes();
        while i < bytes.len() {
            if block[i..].starts_with(".route(") {
                i += ".route(".len();
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                assert_eq!(
                    bytes.get(i),
                    Some(&b'"'),
                    "expected path string after .route("
                );
                i += 1;
                let path_start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                let path = &block[path_start..i];
                i += 1; // closing quote
                while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
                    i += 1;
                }
                let mut depth = 1usize;
                let methods_start = i;
                while i < bytes.len() && depth > 0 {
                    match bytes[i] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    if depth > 0 {
                        i += 1;
                    }
                }
                let methods_src = &block[methods_start..i];
                for (token, method) in [
                    ("get(", "GET"),
                    ("post(", "POST"),
                    ("put(", "PUT"),
                    ("patch(", "PATCH"),
                    ("delete(", "DELETE"),
                ] {
                    if methods_src.contains(token) {
                        routes.push((method, path));
                    }
                }
            } else {
                i += 1;
            }
        }
        assert!(
            !routes.is_empty(),
            "parsed zero .route() registrations from router_with_policy"
        );
        routes
    }

    #[test]
    fn bind_guard_allows_loopback_by_default() {
        let addr: SocketAddr = "127.0.0.1:7878".parse().unwrap();
        assert!(validate_bind(addr, false).is_ok());
    }

    #[test]
    fn bind_guard_rejects_non_loopback_without_opt_in() {
        let addr: SocketAddr = "0.0.0.0:7878".parse().unwrap();
        assert_eq!(
            validate_bind(addr, false),
            Err(ApiConfigError::NonLoopbackBind(addr))
        );
    }

    #[test]
    fn bind_guard_allows_non_loopback_with_opt_in() {
        let addr: SocketAddr = "0.0.0.0:7878".parse().unwrap();
        assert!(validate_bind(addr, true).is_ok());
    }

    #[test]
    fn token_validation_rejects_empty_short_and_defaults() {
        assert!(matches!(
            auth::validate_legacy_token(""),
            Err(auth::AuthError::MissingAuth)
        ));
        assert!(matches!(
            auth::validate_legacy_token("short"),
            Err(auth::AuthError::InvalidLegacyToken)
        ));
        assert!(matches!(
            auth::validate_legacy_token("changeme"),
            Err(auth::AuthError::InvalidLegacyToken)
        ));
    }

    #[test]
    fn token_validation_accepts_long_token() {
        assert!(auth::validate_legacy_token(TOKEN).is_ok());
    }

    #[tokio::test]
    async fn health_works_without_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["status"], "ok");
    }

    #[tokio::test]
    async fn ready_works_without_token_when_no_gate() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["status"], "ready");
        let data = body["data"].as_object().expect("data object");
        assert_eq!(data.keys().collect::<Vec<_>>(), vec!["status"]);
    }

    #[tokio::test]
    async fn ready_returns_503_when_gate_not_ready() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let gate = ReadinessGate::new(true, false, true, false);
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::all(),
            DeployPolicy::default(),
            Some(gate),
            BODY_LIMIT_BYTES,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response_json(response).await;
        assert_eq!(body["data"]["status"], "not_ready");
    }

    #[tokio::test]
    async fn admin_status_requires_scope_and_exposes_reload_without_secrets() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let admin_plain = auth::test_token_plaintext("ops-admin");
        let admin_hash = auth::hash_token(&admin_plain).unwrap();
        let reader_plain = auth::test_token_plaintext("reader");
        let reader_hash = auth::hash_token(&reader_plain).unwrap();
        let tokens_path = dir.path().join("tokens.toml");
        std::fs::write(
            &tokens_path,
            format!(
                r#"
version = 1
[[tokens]]
id = "ops-admin"
hash = "{admin_hash}"
scopes = ["admin:status"]
enabled = true
[[tokens]]
id = "reader"
hash = "{reader_hash}"
scopes = ["scripts:read"]
enabled = true
"#
            ),
        )
        .unwrap();
        let auth = Authenticator::from_tokens_file(&tokens_path).unwrap();
        let gate = ReadinessGate::new(true, false, true, false);
        gate.set_workers_alive(true);
        let app = router_with_policy(
            auth.clone(),
            workspace,
            ApiPolicy::all(),
            DeployPolicy::default(),
            Some(gate),
            BODY_LIMIT_BYTES,
        );

        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/status")
                    .header(header::AUTHORIZATION, format!("Bearer {reader_plain}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        let ok = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/admin/status")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_plain}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body = response_json(ok).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["ready"], true);
        assert_eq!(body["data"]["auth"]["mode"], "tokens_file");
        assert_eq!(body["data"]["auth"]["token_count"], 2);
        let serialized = body.to_string();
        assert!(!serialized.contains(&admin_plain));
        assert!(!serialized.contains(&reader_plain));
        assert!(!serialized.contains("ops-admin"));
        assert!(!serialized.contains("reader"));
        assert!(!serialized.contains(tokens_path.to_string_lossy().as_ref()));

        // Failed reload keeps last valid set and surfaces status.
        std::fs::write(&tokens_path, "not-valid-toml [[[").unwrap();
        assert!(auth.reload().is_err());
        let after = auth.status();
        assert_eq!(after.last_reload_ok, Some(false));
        assert!(auth.authenticate(&admin_plain).is_some());
    }

    #[tokio::test]
    async fn authenticated_mutating_request_emits_audit_with_token_id_redacted_auth() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let plaintext = auth::test_token_plaintext("ci-enqueue");
        let hash = auth::hash_token(&plaintext).unwrap();
        let tokens_path = dir.path().join("tokens.toml");
        std::fs::write(
            &tokens_path,
            format!(
                r#"
version = 1
[[tokens]]
id = "ci-enqueue"
hash = "{hash}"
scopes = ["runs:enqueue", "runs:read"]
enabled = true
"#
            ),
        )
        .unwrap();
        let sink = AuditCapture::install().await;
        let app = router_with_policy(
            Authenticator::from_tokens_file(&tokens_path).unwrap(),
            workspace,
            ApiPolicy::all(),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );
        let request_body = r#"{"script":"job.sh","run_id":"rid-audit","actor":"agent","reason":"request-secret-marker"}"#;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/runs")
                    .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let events = sink.events();
        let event = events
            .iter()
            .find(|e| e.run_id.as_deref() == Some("rid-audit"))
            .expect("audit event for POST /v1/runs");
        assert_eq!(event.token_id.as_deref(), Some("ci-enqueue"));
        assert_eq!(event.run_id.as_deref(), Some("rid-audit"));
        assert_eq!(event.outcome, "ok");
        assert_eq!(event.status, 200);
        let serialized = serde_json::to_string(event).unwrap();
        assert!(!serialized.contains(&plaintext));
        assert!(!serialized.contains("Authorization"));
        assert!(!serialized.to_lowercase().contains("bearer "));
        assert!(!serialized.contains("request-secret-marker"));

        let duplicate = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/runs")
                    .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(request_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let duplicate_status = duplicate.status();
        assert!(!duplicate_status.is_success());
        assert!(sink.events().iter().any(|event| {
            event.run_id.as_deref() == Some("rid-audit")
                && event.status == duplicate_status.as_u16()
        }));
    }

    #[tokio::test]
    async fn rejected_enqueue_audit_keeps_safe_run_id_without_request_secrets() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_secret_script(workspace.scripts_root(), "secret.sh", None);
        let mut deploy = DeployPolicy::default();
        deploy.runs.allow_secret_fields = false;
        let sink = AuditCapture::install().await;
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::all(),
            deploy,
            None,
            BODY_LIMIT_BYTES,
        );

        let response = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-denied-audit","args":["--token","secret://request-secret-marker/token"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let event = sink
            .events()
            .into_iter()
            .find(|event| event.run_id.as_deref() == Some("rid-denied-audit"))
            .expect("audit event for rejected enqueue");
        assert_eq!(event.run_id.as_deref(), Some("rid-denied-audit"));
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains("request-secret-marker"));
        assert!(!serialized.contains(TOKEN));
    }

    #[tokio::test]
    async fn unauthorized_request_emits_audit_without_token_id() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let sink = AuditCapture::install().await;
        let app = router(TOKEN.to_string(), workspace);
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/scripts")
                    .header(header::AUTHORIZATION, "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let events = sink.events();
        let event = events
            .iter()
            .find(|e| e.path == "/v1/scripts" && e.outcome == "unauthorized")
            .expect("401 audit event");
        assert_eq!(event.token_id, None);
        assert_eq!(event.run_id, None);
        assert_eq!(event.status, 401);
        let serialized = serde_json::to_string(event).unwrap();
        assert!(!serialized.contains("wrong-token"));
        assert!(!serialized.contains("Authorization"));
    }

    #[tokio::test]
    async fn legacy_admin_status_requires_explicit_capability() {
        let dir = TempDir::new().unwrap();
        let denied = router_with_auth(
            Authenticator::legacy(TOKEN),
            workspace_in(&dir),
            ApiPolicy::allow([ApiCapability::ScriptsRead]),
        )
        .oneshot(authed_request("/v1/admin/status"))
        .await
        .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);

        let ok = router_with_auth(
            Authenticator::legacy(TOKEN),
            workspace_in(&dir),
            ApiPolicy::allow([ApiCapability::AdminStatus]),
        )
        .oneshot(authed_request("/v1/admin/status"))
        .await
        .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }

    #[test]
    fn capability_all_preserves_explicit_secret_refs() {
        let deny = ApiPolicy::from_config(&["all".into()], &[]).unwrap();
        let access = deny.secret_access(
            &AuthContext {
                token_id: "legacy".into(),
                scopes: vec!["*".into()],
            },
            false,
        );
        assert!(crate::secrets::check_secret_access("secret://prod/token", &access).is_err());

        let allow = ApiPolicy::from_config(&["all".into()], &["*".into()]).unwrap();
        let access = allow.secret_access(
            &AuthContext {
                token_id: "legacy".into(),
                scopes: vec!["*".into()],
            },
            false,
        );
        assert!(crate::secrets::check_secret_access("secret://prod/token", &access).is_ok());
    }

    #[tokio::test]
    async fn ready_remains_minimal_without_token_ids_after_admin_exists() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let gate = ReadinessGate::new(false, false, false, false);
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::all(),
            DeployPolicy::default(),
            Some(gate),
            BODY_LIMIT_BYTES,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let data = body["data"].as_object().expect("data object");
        assert_eq!(data.keys().collect::<Vec<_>>(), vec!["status"]);
        assert!(!body.to_string().contains("token"));
        assert!(!body.to_string().contains("legacy"));
    }

    #[tokio::test]
    async fn protected_route_rejects_missing_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/scripts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn protected_route_rejects_invalid_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/scripts")
                    .header(header::AUTHORIZATION, "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_route_accepts_valid_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/unknown"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn tokens_file_unknown_token_is_401_missing_scope_is_403() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let plaintext = auth::test_token_plaintext("reader");
        let hash = auth::hash_token(&plaintext).unwrap();
        let path = dir.path().join("tokens.toml");
        std::fs::write(
            &path,
            format!(
                r#"
version = 1
[[tokens]]
id = "reader"
hash = "{hash}"
scopes = ["scripts:read"]
enabled = true
"#
            ),
        )
        .unwrap();
        let auth = Authenticator::from_tokens_file(&path).unwrap();
        // Process-wide capabilities would allow runs:write; file mode must ignore them.
        let app = router_with_auth(auth, workspace, ApiPolicy::all());

        let unknown = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/scripts")
                    .header(
                        header::AUTHORIZATION,
                        "Bearer omk_live_not_a_real_token_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);

        let forbidden = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/runs")
                    .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let body = response_json(forbidden).await;
        assert_eq!(body["error"]["code"], "forbidden");

        let allowed = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/scripts")
                    .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tokens_file_env_scope_alias_envs_read() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let plaintext = auth::test_token_plaintext("env-reader");
        let hash = auth::hash_token(&plaintext).unwrap();
        let path = dir.path().join("tokens.toml");
        std::fs::write(
            &path,
            format!(
                r#"
version = 1
[[tokens]]
id = "env-reader"
hash = "{hash}"
scopes = ["envs:read"]
enabled = true
"#
            ),
        )
        .unwrap();
        let app = router_with_auth(
            Authenticator::from_tokens_file(&path).unwrap(),
            workspace,
            ApiPolicy::allow([]),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/envs")
                    .header(header::AUTHORIZATION, format!("Bearer {plaintext}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn read_endpoints_require_explicit_read_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        for uri in [
            "/v1/config",
            "/v1/workspace",
            "/v1/doctor",
            "/v1/scripts",
            "/v1/tree",
            "/v1/runs",
            "/v1/queue/stats",
            "/v1/batteries",
        ] {
            let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "uri: {uri}");
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn read_endpoints_accept_matching_read_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([
                ApiCapability::ConfigRead,
                ApiCapability::ScriptsRead,
                ApiCapability::RunRead,
                ApiCapability::BatteryRead,
            ]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        for uri in [
            "/v1/config",
            "/v1/workspace",
            "/v1/doctor",
            "/v1/scripts",
            "/v1/tree",
            "/v1/runs",
            "/v1/queue/stats",
            "/v1/batteries",
        ] {
            let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "uri: {uri}");
        }
    }

    #[tokio::test]
    async fn workspace_endpoint_returns_summary() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/workspace"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["version"], app_meta::APP_VERSION);
    }

    #[tokio::test]
    async fn config_endpoint_returns_full_masked_config() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(workspace.envs_dir().join("dev.conf"), "HOST=localhost\n").unwrap();
        std::fs::write(workspace.envs_active_path(), "dev.conf\n").unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/config"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["version"], app_meta::APP_VERSION);
        assert_eq!(body["data"]["active_env"], "dev.conf");
        assert_eq!(body["data"]["active_env_keys"][0]["key"], "HOST");
        assert_eq!(body["data"]["active_env_keys"][0]["value"], "****");
        assert!(!body.to_string().contains("localhost"));
    }

    #[tokio::test]
    async fn doctor_endpoint_returns_structured_report() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/doctor"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert!(body["data"]["dependencies"].is_array());
        assert!(body["data"]["workspace_paths"].is_array());
        assert_eq!(body["data"]["schemas"]["total"], 1);
    }

    #[tokio::test]
    async fn scripts_and_schema_endpoints_return_operation_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let app = router(TOKEN.to_string(), workspace);
        let list = app
            .clone()
            .oneshot(authed_request("/v1/scripts?tag=ops"))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["relative_path"], "job.sh");

        let show = app
            .clone()
            .oneshot(authed_request("/v1/scripts/job.sh"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"]["schema"]["name"], "job.sh");

        let schema = app
            .oneshot(authed_request("/v1/scripts/job.sh/schema"))
            .await
            .unwrap();
        assert_eq!(schema.status(), StatusCode::OK);
        let schema_body = response_json(schema).await;
        assert_eq!(schema_body["data"]["tags"][0], "ops");
    }

    #[tokio::test]
    async fn search_endpoint_returns_operation_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "deploy.sh");
        search_ops::search_scripts(
            &workspace,
            search_ops::SearchScriptsRequest {
                query: "deploy".into(),
                tags: vec!["ops".into()],
                refresh: true,
            },
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/search?q=deploy&tag=ops"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"][0]["relative_path"], "deploy.sh");
    }

    #[tokio::test]
    async fn search_endpoint_requires_query() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/search"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn search_endpoint_rejects_empty_and_oversized_query() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        let empty = app
            .clone()
            .oneshot(authed_request("/v1/search?q="))
            .await
            .unwrap();
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

        let too_long = "x".repeat(MAX_SEARCH_QUERY_LEN + 1);
        let long = app
            .oneshot(authed_request(&format!("/v1/search?q={too_long}")))
            .await
            .unwrap();
        assert_eq!(long.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn search_endpoint_rejects_excessive_tags() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        let too_many_tags = (0..=MAX_SEARCH_TAGS)
            .map(|idx| format!("tag=t{idx}"))
            .collect::<Vec<_>>()
            .join("&");
        let too_many = app
            .clone()
            .oneshot(authed_request(&format!(
                "/v1/search?q=deploy&{too_many_tags}"
            )))
            .await
            .unwrap();
        assert_eq!(too_many.status(), StatusCode::BAD_REQUEST);

        let too_long_tag = "x".repeat(MAX_SEARCH_TAG_LEN + 1);
        let long = app
            .oneshot(authed_request(&format!(
                "/v1/search?q=deploy&tag={too_long_tag}"
            )))
            .await
            .unwrap();
        assert_eq!(long.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn script_routes_support_nested_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "tools/job.sh");
        std::fs::write(
            workspace.scripts_root().join("tools/secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Default":"schema_secret_default","Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let show = app
            .clone()
            .oneshot(authed_request("/v1/scripts/tools/job.sh"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"]["relative_path"], "tools/job.sh");

        let schema = app
            .clone()
            .oneshot(authed_request("/v1/scripts/tools/job.sh/schema"))
            .await
            .unwrap();
        assert_eq!(schema.status(), StatusCode::OK);
        let schema_body = response_json(schema).await;
        assert_eq!(schema_body["data"]["name"], "tools/job.sh");

        let secret_schema = app
            .oneshot(authed_request("/v1/scripts/tools/secret.sh/schema"))
            .await
            .unwrap();
        assert_eq!(secret_schema.status(), StatusCode::OK);
        let secret_body = response_json(secret_schema).await;
        assert!(!secret_body.to_string().contains("schema_secret_default"));
    }

    #[tokio::test]
    async fn tree_and_content_endpoints_return_safe_browsing_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "tools/job.sh");

        let app = router(TOKEN.to_string(), workspace);
        let tree = app
            .clone()
            .oneshot(authed_request("/v1/tree"))
            .await
            .unwrap();
        assert_eq!(tree.status(), StatusCode::OK);
        let tree_body = response_json(tree).await;
        assert_eq!(tree_body["data"][0]["kind"], "directory");
        assert_eq!(tree_body["data"][0]["relative_path"], "tools");

        let nested = app
            .clone()
            .oneshot(authed_request("/v1/tree/tools"))
            .await
            .unwrap();
        assert_eq!(nested.status(), StatusCode::OK);
        let nested_body = response_json(nested).await;
        assert_eq!(nested_body["data"][0]["relative_path"], "tools/job.sh");

        let content = app
            .oneshot(authed_request("/v1/scripts/tools/job.sh/content"))
            .await
            .unwrap();
        assert_eq!(content.status(), StatusCode::OK);
        let content_body = response_json(content).await;
        assert_eq!(content_body["data"]["relative_path"], "tools/job.sh");
        assert!(content_body["data"]["content"]
            .as_str()
            .unwrap()
            .contains("echo ok"));
    }

    #[tokio::test]
    async fn content_endpoint_rejects_path_traversal() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/../secret.sh/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsafe_path");
    }

    #[tokio::test]
    async fn content_endpoint_rejects_absolute_encoded_path() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/%2Ftmp%2Fsecret.sh/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsafe_path");
    }

    #[tokio::test]
    async fn tree_and_content_endpoints_reject_metadata_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        for uri in [
            "/v1/tree/.omakure",
            "/v1/tree/.history",
            "/v1/tree/.git",
            "/v1/scripts/.omakure/secret.sh/content",
            "/v1/scripts/.history/secret.sh/content",
            "/v1/scripts/.git/secret.sh/content",
        ] {
            let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "unsafe_path");
        }
    }

    #[tokio::test]
    async fn content_endpoint_error_hides_local_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let root = workspace.root().display().to_string();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/../secret.sh/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert!(!body.to_string().contains(&root));
    }

    #[tokio::test]
    async fn content_endpoint_maps_unsupported_script_to_415() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(workspace.scripts_root().join("note.txt"), "hello\n").unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/note.txt/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsupported_script");
    }

    #[tokio::test]
    async fn scripts_query_percent_decodes_values() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts?tag=ops%20team"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn scripts_endpoint_maps_missing_script_to_404() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/missing.sh"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn env_endpoints_round_trip_and_redact_values() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace.clone_for_executor());

        let create = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/envs",
                r#"{"name":"prod","params":[{"key":"HOST","value":"prod.example.com"},{"key":"API_KEY","value":"super_secret"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);

        let show = app
            .clone()
            .oneshot(authed_request("/v1/envs/prod"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"][0]["key"], "HOST");
        assert_eq!(show_body["data"][1]["key"], "API_KEY");
        assert_eq!(show_body["data"][1]["value"], "****");
        assert!(!show_body.to_string().contains("super_secret"));

        let set = app
            .clone()
            .oneshot(authed_json_method_request(
                Method::PUT,
                "/v1/envs/prod/params/PORT",
                r#"{"value":"443"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(set.status(), StatusCode::OK);

        let activate = app
            .clone()
            .oneshot(authed_json_request("/v1/envs/prod/activate", r#"{}"#))
            .await
            .unwrap();
        assert_eq!(activate.status(), StatusCode::OK);

        let list = app
            .clone()
            .oneshot(authed_request("/v1/envs"))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["name"], "prod");
        assert_eq!(list_body["data"][0]["file"], "prod.conf");
        assert_eq!(list_body["data"][0]["active"], true);

        let remove_param = app
            .clone()
            .oneshot(authed_delete_request("/v1/envs/prod/params/API_KEY"))
            .await
            .unwrap();
        assert_eq!(remove_param.status(), StatusCode::OK);

        let deactivate = app
            .clone()
            .oneshot(authed_delete_request("/v1/envs/active"))
            .await
            .unwrap();
        assert_eq!(deactivate.status(), StatusCode::OK);

        let delete = app
            .oneshot(authed_delete_request("/v1/envs/prod"))
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::OK);
        assert!(!workspace.envs_dir().join("prod.conf").exists());
    }

    #[tokio::test]
    async fn env_endpoints_replace_patch_and_reject_conf_route_names() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        let replace = app
            .clone()
            .oneshot(authed_json_method_request(
                Method::PUT,
                "/v1/envs/dev",
                r#"{"params":[{"key":"HOST","value":"localhost"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(replace.status(), StatusCode::OK);

        let patch = app
            .clone()
            .oneshot(authed_json_method_request(
                Method::PATCH,
                "/v1/envs/dev",
                r#"{"params":[{"key":"HOST","value":"127.0.0.1"},{"key":"TOKEN","value":"secret_value"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(patch.status(), StatusCode::OK);

        let show = app
            .clone()
            .oneshot(authed_request("/v1/envs/dev"))
            .await
            .unwrap();
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"][0]["value"], "127.0.0.1");
        assert_eq!(show_body["data"][1]["value"], "****");
        assert!(!show_body.to_string().contains("secret_value"));

        let invalid = app
            .oneshot(authed_request("/v1/envs/dev.conf"))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let invalid_body = response_json(invalid).await;
        assert_eq!(invalid_body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn env_endpoints_require_specific_policy_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        env_ops::create_env(
            &workspace,
            "prod",
            &[env_ops::EnvParam {
                key: "HOST".into(),
                value: "prod.example.com".into(),
            }],
        )
        .unwrap();
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::EnvRead]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        let read = app
            .clone()
            .oneshot(authed_request("/v1/envs/prod"))
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);

        for request in [
            authed_json_method_request(
                Method::PATCH,
                "/v1/envs/prod",
                r#"{"params":[{"key":"HOST","value":"changed"}]}"#,
            ),
            authed_json_request("/v1/envs/prod/activate", r#"{}"#),
            authed_delete_request("/v1/envs/active"),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn runs_and_queue_stats_endpoints_return_operation_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        runs::enqueue(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            EnqueueOptions {
                run_id: Some("rid-http".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let list = app
            .clone()
            .oneshot(authed_request("/v1/runs?state_set=all"))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["run_id"], "rid-http");

        let show = app
            .clone()
            .oneshot(authed_request("/v1/runs/rid-http"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"]["actor"], "agent");

        let traces = app
            .clone()
            .oneshot(authed_request("/v1/runs/rid-http/traces"))
            .await
            .unwrap();
        assert_eq!(traces.status(), StatusCode::OK);

        let stats = app
            .oneshot(authed_request("/v1/queue/stats"))
            .await
            .unwrap();
        assert_eq!(stats.status(), StatusCode::OK);
        let stats_body = response_json(stats).await;
        assert_eq!(stats_body["data"]["total"], 1);
    }

    #[tokio::test]
    async fn runs_endpoint_maps_invalid_query_to_400() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/runs?state=bad"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn enqueue_run_requires_auth() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"script":"job.sh"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn enqueue_run_returns_queued_run() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"job","args":["--x"],"run_id":"rid-post","actor":"agent","reason":"api","priority":7,"timeout_ms":1000}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["run_id"], "rid-post");
        assert_eq!(body["data"]["state"], "queued");
        assert_eq!(body["data"]["actor"], "agent");
        assert_eq!(body["data"]["priority"], 7);
    }

    #[tokio::test]
    async fn enqueue_run_endpoint_redacts_secret_args_in_response_and_storage() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "token=http_secret_value\n",
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace.clone_for_executor())
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-secret","args":["--token","secret://prod/token"]}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let rendered = body.to_string();
        assert!(rendered.contains("secret://prod/token"));
        assert!(!rendered.contains("http_secret_value"));

        let conn = runs::open(&workspace).unwrap();
        let row = runs::get_run(&conn, "rid-http-secret").unwrap().unwrap();
        assert!(row.args_json.contains("secret://prod/token"));
        assert!(!row.args_json.contains("http_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_rejects_plaintext_secret_arg_values() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-plain-secret","args":["--token","http_secret_value"]}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
        assert!(!body.to_string().contains("http_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_accepts_env_and_rejects_non_reconstructable_secret_fields() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"},{"Name":"MODE","Type":"string","Arg":"--mode"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=env_secret_value\n",
        )
        .unwrap();

        let env_response = router(TOKEN.to_string(), workspace.clone_for_executor())
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-env-secret","args":["--mode","fast"],"env":"prod"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(env_response.status(), StatusCode::OK);
        let env_body = response_json(env_response).await;
        assert!(env_body.to_string().contains("<redacted>"));
        assert!(!env_body.to_string().contains("env_secret_value"));

        let conn = runs::open(&workspace).unwrap();
        let env_row = runs::get_run(&conn, "rid-http-env-secret")
            .unwrap()
            .unwrap();
        assert_eq!(
            runs::get_run_env(&conn, "rid-http-env-secret")
                .unwrap()
                .as_deref(),
            Some("prod")
        );
        assert!(env_row.args_json.contains("<redacted>"));
        assert!(!env_row.args_json.contains("env_secret_value"));
        drop(conn);

        let direct_response = router(TOKEN.to_string(), workspace.clone_for_executor())
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-direct-secret","secret_fields":{"TOKEN":"direct_secret_value"}}"#,
            ))
            .await
            .unwrap();

        assert_eq!(direct_response.status(), StatusCode::BAD_REQUEST);
        let direct_body = response_json(direct_response).await;
        assert_eq!(direct_body["error"]["code"], "invalid_input");
        assert!(!direct_body.to_string().contains("direct_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_enforces_secret_provider_acl() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "token=provider_secret\n",
        )
        .unwrap();
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace.clone_for_executor(),
            ApiPolicy::allow_with_secret_refs(
                [ApiCapability::RunWrite, ApiCapability::SecretProviderUse],
                ["secret://prod/other"],
            ),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        let denied = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-denied-ref","args":["--token","secret://prod/token"]}"#,
            ))
            .await
            .unwrap();

        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let body = response_json(denied).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(!body.to_string().contains("provider_secret"));
    }

    #[tokio::test]
    async fn enqueue_run_env_and_secret_fields_require_policy_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=env_secret_value\n",
        )
        .unwrap();
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        let plain = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"job.sh","run_id":"rid-plain"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(plain.status(), StatusCode::OK);

        for body in [
            r#"{"script":"job.sh","run_id":"rid-env","env":"prod"}"#,
            r#"{"script":"job.sh","run_id":"rid-secret","secret_fields":{"TOKEN":"direct_secret_value"}}"#,
            r#"{"script":"job.sh","run_id":"rid-secret-ref","args":["--token","secret://prod/token"]}"#,
        ] {
            let response = app
                .clone()
                .oneshot(authed_json_request("/v1/runs", body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn enqueue_run_secret_fields_policy_denies_all_provider_entry_points() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_secret_script(workspace.scripts_root(), "secret.sh", None);
        write_secret_script(
            workspace.scripts_root(),
            "secret-default.sh",
            Some("secret://prod/token"),
        );
        let mut deploy = DeployPolicy::default();
        deploy.runs.allow_secret_fields = false;
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace.clone_for_executor(),
            ApiPolicy::all(),
            deploy,
            None,
            BODY_LIMIT_BYTES,
        );

        for (run_id, body) in [
            (
                "rid-policy-args",
                r#"{"script":"secret.sh","run_id":"rid-policy-args","args":["--token","secret://prod/token"]}"#,
            ),
            (
                "rid-policy-fields",
                r#"{"script":"secret.sh","run_id":"rid-policy-fields","secret_fields":{"TOKEN":"secret://prod/token"}}"#,
            ),
            (
                "rid-policy-default",
                r#"{"script":"secret-default.sh","run_id":"rid-policy-default"}"#,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(authed_json_request("/v1/runs", body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{run_id}");
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
            assert_eq!(
                body["error"]["message"],
                "policy runs.allow_secret_fields=false"
            );
        }

        let conn = runs::open(&workspace).unwrap();
        for run_id in ["rid-policy-args", "rid-policy-fields", "rid-policy-default"] {
            assert!(runs::get_run(&conn, run_id).unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn enqueue_run_implicit_secret_default_requires_secret_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_secret_script(
            workspace.scripts_root(),
            "secret-default.sh",
            Some("schema_secret_value"),
        );
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        let response = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret-default.sh","run_id":"rid-default"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(!body.to_string().contains("schema_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_implicit_active_env_secret_requires_env_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_secret_script(workspace.scripts_root(), "secret-env.sh", None);
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "token=active_secret\n",
        )
        .unwrap();
        std::fs::write(workspace.envs_active_path(), "prod.conf\n").unwrap();
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        let response = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret-env.sh","run_id":"rid-env-implicit"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(!body.to_string().contains("active_secret"));
    }

    #[tokio::test]
    async fn mutating_run_routes_require_run_write_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let conn = runs::open(&workspace).unwrap();
        runs::enqueue(
            &conn,
            "job.sh",
            &[],
            EnqueueOptions {
                run_id: Some("rid-cap".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::EnvRead]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        for request in [
            authed_json_request("/v1/runs/rid-cap/cancel", r#"{}"#),
            authed_json_request("/v1/runs/rid-cap/dead-letter", r#"{}"#),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_authentications_recycle_permits_and_do_not_deadlock() {
        // Fire more concurrent requests than the bounded auth budget. Requests
        // either authenticate or fail fast; none may queue indefinitely.
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);
        let mut handles = Vec::new();
        for _ in 0..24 {
            let app = app.clone();
            handles.push(tokio::spawn(async move {
                app.oneshot(authed_request("/v1/config"))
                    .await
                    .unwrap()
                    .status()
            }));
        }
        let mut accepted = 0;
        for handle in handles {
            match handle.await.unwrap() {
                StatusCode::OK => accepted += 1,
                StatusCode::SERVICE_UNAVAILABLE => {}
                status => panic!("unexpected auth response: {status}"),
            }
        }
        assert!(accepted > 0);
    }

    #[tokio::test]
    async fn mutating_battery_routes_require_battery_write_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "azure".into(),
                git_url: "https://example.invalid/azure.git".into(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();
        let app = router_with_policy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::EnvRead]),
            DeployPolicy::default(),
            None,
            BODY_LIMIT_BYTES,
        );

        for request in [
            authed_json_request(
                "/v1/batteries",
                r#"{"name":"new","git_url":"https://example.invalid/new.git"}"#,
            ),
            authed_json_request("/v1/batteries/azure/sync", r#"{}"#),
            authed_json_request("/v1/batteries/azure/scripts/list/install", r#"{}"#),
            authed_delete_request("/v1/batteries/azure"),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn enqueue_run_maps_invalid_script_to_404() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"missing.sh"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn enqueue_run_rejects_outside_workspace_script() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(outside.path(), "outside.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                &format!(
                    r#"{{"script":"{}"}}"#,
                    outside.path().join("outside.sh").display()
                ),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsafe_path");
    }

    #[tokio::test]
    async fn malformed_json_returns_envelope() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/runs", r#"{"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn oversized_json_returns_envelope() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let body = format!(r#"{{"script":"{}"}}"#, "x".repeat(BODY_LIMIT_BYTES + 1));

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/runs", &body))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "payload_too_large");
    }

    #[tokio::test]
    async fn cancel_run_success_and_missing_run() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        runs::enqueue(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            EnqueueOptions {
                run_id: Some("rid-cancel".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let cancelled = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/runs/rid-cancel/cancel",
                r#"{"reason":"stop"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::OK);
        let cancelled_body = response_json(cancelled).await;
        assert_eq!(cancelled_body["data"]["state"], "cancelled");

        let missing = app
            .oneshot(authed_json_request("/v1/runs/missing/cancel", r#"{}"#))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_run_maps_invalid_transition_to_conflict() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        let row = runs::start_inline(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            "worker:test",
            EnqueueOptions {
                run_id: Some("rid-done".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();
        runs::complete(
            &conn,
            &row.run_id,
            RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                success: true,
                error: None,
            },
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/runs/rid-done/cancel", r#"{}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "conflict");
    }

    #[tokio::test]
    async fn dead_letter_run_success_and_invalid_transition() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        let failed = runs::start_inline(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            "worker:test",
            EnqueueOptions {
                run_id: Some("rid-failed".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();
        runs::fail(
            &conn,
            &failed.run_id,
            RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(1),
                success: false,
                error: Some("boom".into()),
            },
        )
        .unwrap();
        runs::enqueue(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            EnqueueOptions {
                run_id: Some("rid-queued".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let promoted = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/runs/rid-failed/dead-letter",
                r#"{"reason":"triaged"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(promoted.status(), StatusCode::OK);
        let promoted_body = response_json(promoted).await;
        assert_eq!(promoted_body["data"]["state"], "dead_letter");

        let invalid = app
            .oneshot(authed_json_request(
                "/v1/runs/rid-queued/dead-letter",
                r#"{}"#,
            ))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::CONFLICT);
        let invalid_body = response_json(invalid).await;
        assert_eq!(invalid_body["error"]["code"], "conflict");
    }

    #[tokio::test]
    async fn battery_add_with_token_ref_stores_auth_metadata_only() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "GIT_TOKEN=never-persist-this-plaintext-token\n",
        )
        .unwrap();
        let mut deploy = DeployPolicy::default();
        deploy.sources.allow_private_https_batteries = true;
        let app = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace.clone_for_executor(),
            ApiPolicy::allow_with_secret_refs(
                [
                    ApiCapability::BatteryWrite,
                    ApiCapability::BatteryRead,
                    ApiCapability::CredentialsUse,
                ],
                ["secret://prod/GIT_TOKEN"],
            ),
            deploy,
        );
        let add = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"private","git_url":"https://example.invalid/private.git","requested_ref":"main","token_ref":"secret://prod/GIT_TOKEN"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let add_body = response_json(add).await;
        assert_eq!(add_body["data"]["auth"]["method"], "https_token_ref");
        assert_eq!(
            add_body["data"]["auth"]["token_ref"],
            "secret://prod/GIT_TOKEN"
        );
        let list = app.oneshot(authed_request("/v1/batteries")).await.unwrap();
        let body = response_json(list).await;
        let auth = &body["data"][0]["auth"];
        assert_eq!(auth["method"], "https_token_ref");
        assert_eq!(auth["token_ref"], "secret://prod/GIT_TOKEN");
        let registry = std::fs::read_to_string(
            battery_ops::BatteryPaths::for_workspace(&workspace).registry_path,
        )
        .unwrap();
        assert!(!registry.contains("never-persist-this-plaintext-token"));
        assert!(registry.contains("secret://prod/GIT_TOKEN"));
    }

    #[tokio::test]
    async fn battery_add_token_ref_denied_without_credentials_use() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let mut deploy = DeployPolicy::default();
        deploy.sources.allow_private_https_batteries = true;
        let app = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::BatteryWrite]),
            deploy,
        );
        let add = app
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"private","git_url":"https://example.invalid/private.git","token_ref":"secret://prod/token"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn private_https_sync_denied_without_credentials_use() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.envs_dir().join("creds.conf"),
            "git_token=sync-secret-value\n",
        )
        .unwrap();
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "private".into(),
                git_url: "https://example.invalid/private.git".into(),
                requested_ref: "main".into(),
                token_ref: Some("secret://creds/git_token".into()),
            },
        )
        .unwrap();
        let mut deploy = DeployPolicy::default();
        deploy.sources.allow_private_https_batteries = true;
        let app = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::BatteryWrite]),
            deploy,
        );
        let response = app
            .oneshot(authed_json_request("/v1/batteries/private/sync", r#"{}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert!(body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("credentials:use"));
        assert!(!body.to_string().contains("sync-secret-value"));
    }

    #[tokio::test]
    async fn secrets_metadata_endpoint_redacts_values() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let secret_value = "metadata-must-not-leak-this-value";
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            format!("TOKEN={secret_value}\n"),
        )
        .unwrap();
        let mut deploy = DeployPolicy::default();
        deploy.secrets.metadata_endpoint = true;
        let app = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow_with_secret_refs(
                [ApiCapability::SecretsReadMetadata],
                ["secret://prod/*"],
            ),
            deploy,
        );
        let response = app.oneshot(authed_request("/v1/secrets")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let rendered = body.to_string();
        assert!(!rendered.contains(secret_value));
        assert_eq!(body["data"][0]["id"], "secret://prod/token");
        assert!(body["data"][0]["source"]
            .as_str()
            .unwrap()
            .starts_with("file:"));
        assert!(body["data"][0].get("value").is_none());
    }

    #[tokio::test]
    async fn secrets_metadata_requires_scope_and_policy_flag() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let mut deploy = DeployPolicy::default();
        deploy.secrets.metadata_endpoint = false;
        let disabled = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace.clone_for_executor(),
            ApiPolicy::allow([ApiCapability::SecretsReadMetadata]),
            deploy.clone(),
        );
        let response = disabled
            .oneshot(authed_request("/v1/secrets"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        deploy.secrets.metadata_endpoint = true;
        let denied = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::allow([ApiCapability::BatteryRead]),
            deploy,
        );
        let response = denied.oneshot(authed_request("/v1/secrets")).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn battery_endpoints_require_auth() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/batteries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn battery_add_and_list_use_operations() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let app = router(TOKEN.to_string(), workspace);
        let add = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"azure","git_url":"https://example.invalid/azure.git","requested_ref":"stable"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let add_body = response_json(add).await;
        assert_eq!(add_body["data"]["name"], "azure");
        assert_eq!(add_body["data"]["requested_ref"], "stable");

        let list = app.oneshot(authed_request("/v1/batteries")).await.unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["name"], "azure");
    }

    #[tokio::test]
    async fn battery_add_rejects_plaintext_http_git_url() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"plain","git_url":"http://example.invalid/plain.git"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn battery_add_rejects_local_git_url() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"local","git_url":"/tmp/local-battery.git"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn battery_sync_invalid_manifest_maps_to_400() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        register_invalid_https_battery_cache(&workspace, "bad");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/batteries/bad"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "manifest_invalid");
    }

    #[tokio::test]
    async fn battery_http_operations_reject_existing_non_https_sources() {
        let repo = invalid_manifest_repo();
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "local".into(),
                git_url: repo.path().display().to_string(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        for request in [
            authed_json_request("/v1/batteries/local/sync", r#"{}"#),
            authed_request("/v1/batteries/local"),
            authed_request("/v1/batteries/local/scripts"),
            authed_json_request("/v1/batteries/local/scripts/list/install", r#"{}"#),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "invalid_input");
        }
    }

    #[tokio::test]
    async fn battery_missing_and_unsynced_errors_are_stable() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "azure".into(),
                git_url: "https://example.invalid/azure.git".into(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let missing = app
            .clone()
            .oneshot(authed_request("/v1/batteries/missing"))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let missing_body = response_json(missing).await;
        assert_eq!(missing_body["error"]["code"], "not_found");

        let unsynced = app
            .clone()
            .oneshot(authed_request("/v1/batteries/azure/scripts"))
            .await
            .unwrap();
        assert_eq!(unsynced.status(), StatusCode::CONFLICT);
        let unsynced_body = response_json(unsynced).await;
        assert_eq!(unsynced_body["error"]["code"], "not_synced");

        let install = app
            .oneshot(authed_json_request(
                "/v1/batteries/azure/scripts/list/install",
                r#"{}"#,
            ))
            .await
            .unwrap();
        assert_eq!(install.status(), StatusCode::CONFLICT);
        let install_body = response_json(install).await;
        assert_eq!(install_body["error"]["code"], "not_synced");
    }

    #[tokio::test]
    async fn battery_sync_missing_maps_to_404() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/batteries/missing/sync", r#"{}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn battery_remove_supports_cache_flag() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "keep".into(),
                git_url: "https://example.invalid/keep.git".into(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "drop".into(),
                git_url: "https://example.invalid/drop.git".into(),
                requested_ref: "main".into(),
                token_ref: None,
            },
        )
        .unwrap();
        let paths = battery_ops::BatteryPaths::for_workspace(&workspace);
        std::fs::create_dir_all(paths.cache_path_for("drop")).unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let keep = app
            .clone()
            .oneshot(authed_delete_request("/v1/batteries/keep"))
            .await
            .unwrap();
        assert_eq!(keep.status(), StatusCode::OK);
        let keep_body = response_json(keep).await;
        assert_eq!(keep_body["data"]["cache_removed"], false);

        let drop = app
            .oneshot(authed_delete_request(
                "/v1/batteries/drop?remove_cache=true",
            ))
            .await
            .unwrap();
        assert_eq!(drop.status(), StatusCode::OK);
        let drop_body = response_json(drop).await;
        assert_eq!(drop_body["data"]["cache_removed"], true);
    }

    #[tokio::test]
    async fn deploy_policy_writes_false_forbids_writes_even_with_wildcard_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let mut deploy = DeployPolicy::default();
        deploy.routes.writes = false;
        let app = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::all(),
            deploy,
        );

        let read = app
            .clone()
            .oneshot(authed_request("/v1/scripts"))
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);

        let write = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"job.sh","run_id":"policy-ro"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(write.status(), StatusCode::FORBIDDEN);
        let body = response_json(write).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("deployment policy"));
    }

    #[tokio::test]
    async fn deploy_policy_battery_false_forbids_all_battery_routes() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let mut deploy = DeployPolicy::default();
        deploy.routes.battery = false;
        let app = router_with_deploy(
            Authenticator::legacy(TOKEN),
            workspace,
            ApiPolicy::all(),
            deploy,
        );

        for uri in [
            "/v1/batteries",
            "/v1/batteries/azure",
            "/v1/batteries/azure/scripts",
        ] {
            let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "uri={uri}");
        }
        let sync = app
            .oneshot(authed_json_request("/v1/batteries/azure/sync", r#"{}"#))
            .await
            .unwrap();
        assert_eq!(sync.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn prepare_api_boot_rejects_bad_policy_before_bind() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad-policy.toml");
        std::fs::write(&path, "version = 1\nnot = [valid\n").unwrap();
        let args = ApiArgs {
            bind: "127.0.0.1:7878".parse().unwrap(),
            allow_non_loopback: false,
            policy: Some(path),
            tokens_file: None,
            capabilities: vec!["all".into()],
            secret_refs: vec![],
        };
        let err = match prepare_api_boot(&args) {
            Err(e) => e,
            Ok(_) => panic!("expected policy parse failure"),
        };
        assert!(matches!(err, ApiConfigError::Policy(_)), "got {err}");
        assert!(err.to_string().contains("parse"));
    }

    #[test]
    fn prepare_api_boot_legacy_disabled_rejects_env_token() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("policy.toml");
        std::fs::write(&path, "version = 1\n[auth]\nlegacy_env_token = false\n").unwrap();
        // Direct auth path (avoid mutating process-wide OMAKURE_API_TOKEN).
        let err = match resolve_auth_with_policy(None, false) {
            Err(e) => e,
            Ok(_) => panic!("expected legacy disabled failure"),
        };
        assert!(matches!(err, ApiConfigError::Auth(_)), "got {err}");
        assert!(err.to_string().contains("legacy_env_token"));

        // Policy load still succeeds; boot fails at auth when no tokens file.
        let args = ApiArgs {
            bind: "127.0.0.1:7878".parse().unwrap(),
            allow_non_loopback: false,
            policy: Some(path),
            tokens_file: None,
            capabilities: vec![],
            secret_refs: vec![],
        };
        // Clear tokens file env for this check if present.
        let prev_tokens = std::env::var("OMAKURE_TOKENS_FILE").ok();
        std::env::remove_var("OMAKURE_TOKENS_FILE");
        let boot_err = match prepare_api_boot(&args) {
            Err(e) => e,
            Ok(_) => panic!("expected auth failure without tokens file"),
        };
        if let Some(v) = prev_tokens {
            std::env::set_var("OMAKURE_TOKENS_FILE", v);
        }
        assert!(
            matches!(boot_err, ApiConfigError::Auth(_)),
            "got {boot_err}"
        );
    }

    #[test]
    fn prepare_api_boot_policy_allow_non_loopback_permits_bind() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("policy.toml");
        std::fs::write(
            &path,
            r#"
version = 1
[http]
allow_non_loopback = true
bind = "0.0.0.0:7878"
[auth]
legacy_env_token = true
"#,
        )
        .unwrap();
        let tokens = dir.path().join("tokens.toml");
        let plaintext = auth::test_token_plaintext("admin");
        let hash = auth::hash_token(&plaintext).unwrap();
        std::fs::write(
            &tokens,
            format!(
                r#"
version = 1
[[tokens]]
id = "admin"
hash = "{hash}"
scopes = ["*"]
enabled = true
"#
            ),
        )
        .unwrap();
        let args = ApiArgs {
            bind: "127.0.0.1:7878".parse().unwrap(),
            allow_non_loopback: false,
            policy: Some(path),
            tokens_file: Some(tokens),
            capabilities: vec![],
            secret_refs: vec![],
        };
        let boot = prepare_api_boot(&args).unwrap();
        assert_eq!(boot.bind.to_string(), "0.0.0.0:7878");
        assert!(boot.allow_non_loopback);
    }
}
