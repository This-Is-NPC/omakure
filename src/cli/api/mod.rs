use crate::auth;
use crate::cli::args::ApiArgs;
use crate::workspace::Workspace;
use axum::Router;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

mod audit;
mod battery;
mod bearer;
mod blocking;
mod boot;
mod envs;
mod node;
mod query;
mod respond;
mod router;
mod runs;
mod scripts;
mod secrets;
mod state;
mod status;

pub(crate) use boot::{ApiSurfaces, auth_verification_gate, prepare_api_boot, serve_http};
pub(crate) use router::health_plane_router;
pub(crate) use state::ReadinessGate;

const SIGNED_BUNDLE_HTTP_BODY_LIMIT_BYTES: usize = 32 * 1024;

pub fn run(scripts_dir: PathBuf, args: ApiArgs) -> Result<(), Box<dyn Error>> {
    let boot = prepare_api_boot(&args)?;
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let cancel_flag = Arc::new(AtomicBool::new(false));
    crate::adapters::signals::install_signal_handlers(Arc::clone(&cancel_flag));
    auth::install_sighup_reload(boot.auth.clone());

    let auth_verification_gate = auth_verification_gate(&boot.deploy);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        serve_http(
            boot,
            workspace,
            ApiSurfaces {
                readiness: None,
                transport: None,
                discovery: None,
                cues: None,
                baselines: None,
                health_plane: Router::new(),
                bootstrap_token_path: std::env::var_os(
                    crate::operations::node::BOOTSTRAP_TOKEN_FILE_ENV,
                )
                .map(PathBuf::from),
            },
            auth_verification_gate,
            cancel_flag,
        )
        .await
    })
}

#[cfg(test)]
mod tests;
