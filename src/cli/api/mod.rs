use crate::auth;
use crate::cli::args::ApiArgs;
use crate::workspace::Workspace;
use axum::Router;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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

pub(crate) use boot::{auth_verification_gate, prepare_api_boot, serve_http};
pub(crate) use router::health_plane_router;
pub use router::HTTP_ROUTE_INVENTORY;
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
            boot.bind,
            boot.auth,
            workspace,
            boot.api_policy,
            boot.deploy,
            None,
            None,
            None,
            // API-only mode runs no direct transport, so there is no session a
            // Cue or a baseline could travel on.
            None,
            None,
            Router::new(),
            auth_verification_gate,
            cancel_flag,
            None,
        )
        .await
    })
}

#[cfg(test)]
mod tests;
