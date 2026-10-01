use super::router::router_with_transport;
use super::state::{ApiPolicy, ReadinessGate, MAX_CONCURRENT_BLOCKING_OPERATIONS};
use crate::auth::{self, Authenticator};
use crate::cli::args::ApiArgs;
use crate::direct_service::TransportStatusHandle;
use crate::policy::{self, DeployPolicy};
use crate::workspace::Workspace;
use axum::Router;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApiConfigError {
    NonLoopbackBind(SocketAddr),
    Auth(String),
    Policy(String),
}

impl std::fmt::Display for ApiConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonLoopbackBind(addr) => write!(
                f,
                "refusing to bind {addr}; pass --allow-non-loopback to opt in"
            ),
            Self::Auth(msg) => write!(f, "{msg}"),
            Self::Policy(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for ApiConfigError {}

/// Resolved startup config for `api` / `node serve` (validated before bind).
#[derive(Clone)]
pub(crate) struct ApiBoot {
    pub bind: SocketAddr,
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
    let auth = auth::resolve_authenticator(tokens_file.as_deref())
        .map_err(|err| ApiConfigError::Auth(err.to_string()))?;
    let api_policy = ApiPolicy::from_secret_refs(&args.secret_refs);

    Ok(ApiBoot {
        bind,
        auth,
        api_policy,
        deploy,
    })
}

pub(crate) fn auth_verification_gate(deploy: &DeployPolicy) -> Arc<tokio::sync::Semaphore> {
    Arc::new(tokio::sync::Semaphore::new(
        deploy
            .auth
            .max_concurrent_verifications
            .clamp(1, policy::MAX_CONCURRENT_AUTH_VERIFICATIONS),
    ))
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
    transport: Option<TransportStatusHandle>,
    discovery: Option<crate::discovery::DiscoveryStatusHandle>,
    cues: Option<crate::direct_service::CueDispatcher>,
    baselines: Option<crate::direct_service::BaselineDispatcher>,
    health_plane: Router,
    auth_verification_gate: Arc<tokio::sync::Semaphore>,
    cancel_flag: Arc<AtomicBool>,
    on_listening: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<(), Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    if let Some(tx) = on_listening {
        let _ = tx.send(());
    }
    let body_limit = deploy.http.body_limit_bytes.max(1);
    let app = router_with_transport(
        auth,
        workspace,
        policy,
        deploy,
        readiness,
        transport,
        discovery,
        cues,
        baselines,
        auth_verification_gate,
        Arc::new(tokio::sync::Semaphore::new(
            MAX_CONCURRENT_BLOCKING_OPERATIONS,
        )),
        body_limit,
    );
    let app = app.nest("/v1/node", health_plane);
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

pub(crate) fn validate_bind(
    addr: SocketAddr,
    allow_non_loopback: bool,
) -> Result<(), ApiConfigError> {
    if allow_non_loopback || addr.ip().is_loopback() {
        return Ok(());
    }

    Err(ApiConfigError::NonLoopbackBind(addr))
}
