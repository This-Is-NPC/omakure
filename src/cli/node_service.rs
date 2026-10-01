//! `omakure node serve` — HTTP API + optional in-process workers + scheduler.
//!
//! Composes existing `api::serve_http`, `operations::worker::worker_loop`, and
//! `serve::scheduler_tick` under one cancel flag. Shutdown order:
//! stop accepting HTTP → stop scheduling → stop claiming → drain/join workers.

use crate::cli::api::{self, ReadinessGate};
use crate::cli::args::{ApiArgs, NodeServeArgs};
use crate::cli::serve;
use crate::workspace::Workspace;
use chrono::Utc;
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const SCHEDULER_SCAN_SLICE_MS: u64 = 200;
const SCHEDULER_SCAN_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum LoopKind {
    Workers,
    Scheduler,
}

struct LoopLifecycle {
    readiness: Arc<ReadinessGate>,
    kind: LoopKind,
    expected: usize,
    state: Mutex<LoopLifecycleState>,
}

#[derive(Default)]
struct LoopLifecycleState {
    entered: usize,
    exited: bool,
}

struct LoopGuard {
    lifecycle: Arc<LoopLifecycle>,
}

impl LoopLifecycle {
    fn new(readiness: Arc<ReadinessGate>, kind: LoopKind, expected: usize) -> Arc<Self> {
        Arc::new(Self {
            readiness,
            kind,
            expected,
            state: Mutex::new(LoopLifecycleState::default()),
        })
    }

    fn enter(self: &Arc<Self>) -> LoopGuard {
        let mut state = self.state.lock().expect("loop lifecycle lock");
        state.entered += 1;
        if !state.exited && state.entered == self.expected {
            self.set_alive(true);
        }
        drop(state);
        LoopGuard {
            lifecycle: Arc::clone(self),
        }
    }

    fn set_alive(&self, alive: bool) {
        match self.kind {
            LoopKind::Workers => self.readiness.set_workers_alive(alive),
            LoopKind::Scheduler => self.readiness.set_scheduler_alive(alive),
        }
    }
}

impl Drop for LoopGuard {
    fn drop(&mut self) {
        let mut state = self.lifecycle.state.lock().expect("loop lifecycle lock");
        state.exited = true;
        self.lifecycle.set_alive(false);
    }
}

fn run_tracked_loop(lifecycle: Arc<LoopLifecycle>, run_loop: impl FnOnce()) {
    let _guard = lifecycle.enter();
    run_loop();
}
fn start_discovery_service(
    settings: crate::domain::DiscoverySettings,
    context: crate::node::NodeContext,
    direct_port: Option<u16>,
    secret: Option<String>,
) -> Result<Option<crate::discovery::DiscoveryService>, crate::discovery::DiscoveryError> {
    if !settings.enabled {
        return Ok(None);
    }
    match crate::discovery::DiscoveryService::start(settings.clone(), context, direct_port, secret)
    {
        Ok(service) => Ok(Some(service)),
        Err(crate::discovery::DiscoveryError::UnsupportedPlatform) => Ok(Some(
            crate::discovery::DiscoveryService::disabled(settings, false),
        )),
        Err(error) => Err(error),
    }
}

fn resolve_direct_bind(
    override_bind: Option<SocketAddr>,
    configured_bind: Option<&str>,
    allow_non_loopback: bool,
) -> Result<Option<SocketAddr>, Box<dyn Error>> {
    let bind = match (override_bind, configured_bind) {
        (Some(bind), _) => Some(bind),
        (None, Some(bind)) => Some(bind.parse()?),
        (None, None) => None,
    };
    if let Some(bind) = bind {
        if !bind.ip().is_loopback() && !allow_non_loopback {
            return Err(format!(
                "refusing to bind direct transport {bind}; pass --allow-non-loopback-direct to opt in"
            )
            .into());
        }
    }
    Ok(bind)
}

fn scheduler_enabled(no_scheduler: bool, scheduler: bool, configured: Option<bool>) -> bool {
    if no_scheduler {
        false
    } else if scheduler {
        true
    } else {
        configured.unwrap_or(true)
    }
}

fn resolve_discovery_secret(
    config: &crate::domain::NodeConfig,
    workspace: &Workspace,
) -> Result<Option<String>, Box<dyn Error>> {
    if !config.discovery.enabled || config.organization.discovery_secret_ref.is_empty() {
        return Ok(None);
    }
    crate::secrets::resolve_secret_value(
        workspace,
        &config.organization.discovery_secret_ref,
        &crate::secrets::SecretAccess::allow_all(),
    )
    .map(Some)
    .map_err(|_| "discovery_secret_invalid".into())
}

fn spawn_transport_watcher(
    status: Option<&crate::direct_service::TransportStatusHandle>,
    readiness: &Arc<ReadinessGate>,
) -> Option<(Arc<AtomicBool>, thread::JoinHandle<()>)> {
    status.map(|status| {
        let status = Arc::clone(status);
        let readiness = Arc::clone(readiness);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = Arc::clone(&cancel);
        let handle = thread::spawn(move || {
            while !cancel_for_thread.load(Ordering::SeqCst) {
                let connected = status.lock().ok().is_some_and(|status| {
                    status.expected_peer_count == 0
                        || status.expected_connected_peer_count == status.expected_peer_count
                });
                readiness.set_transport_alive(connected);
                thread::sleep(Duration::from_millis(100));
            }
        });
        (cancel, handle)
    })
}

fn spawn_workers(
    workers: u32,
    workspace: &Workspace,
    context: &crate::node::NodeContext,
    args: &NodeServeArgs,
    readiness: &Arc<ReadinessGate>,
    cancel_flag: &Arc<AtomicBool>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    if workers >= 1 {
        let worker_lifecycle =
            LoopLifecycle::new(Arc::clone(readiness), LoopKind::Workers, workers as usize);
        for thread_idx in 0..workers {
            let ws = workspace.clone_for_executor();
            let flag = Arc::clone(cancel_flag);
            let worker_context = context.clone();
            let actor_filter = args.worker_actor_filter.clone();
            let script_filter = args.worker_script_filter.clone();
            let worker_id = format!("node-worker:{}-t{}", std::process::id(), thread_idx);
            let lifecycle = Arc::clone(&worker_lifecycle);
            handles.push(thread::spawn(move || {
                run_tracked_loop(lifecycle, || {
                    crate::operations::worker::worker_loop_with_context(
                        ws,
                        worker_id,
                        flag,
                        actor_filter,
                        script_filter,
                        false,
                        worker_context,
                    );
                });
            }));
        }
    }
    handles
}

fn stop_live_services(
    cancel_flag: &Arc<AtomicBool>,
    transport_watcher: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>,
    direct_service: &mut Option<crate::direct_service::DirectService>,
    discovery_service: &mut Option<crate::discovery::DiscoveryService>,
    readiness: &Arc<ReadinessGate>,
) {
    cancel_flag.store(true, Ordering::SeqCst);
    if let Some((watcher_cancel, watcher)) = transport_watcher {
        watcher_cancel.store(true, Ordering::SeqCst);
        let _ = watcher.join();
    }
    if let Some(service) = direct_service.as_mut() {
        service.stop();
    }
    if let Some(service) = discovery_service.as_mut() {
        service.stop();
    }
    readiness.set_workers_alive(false);
    readiness.set_scheduler_alive(false);
}

fn join_background_loops(
    scheduler_handle: Option<thread::JoinHandle<()>>,
    health_maintenance: thread::JoinHandle<()>,
    worker_handles: Vec<thread::JoinHandle<()>>,
) {
    if let Some(handle) = scheduler_handle {
        let _ = handle.join();
    }
    let _ = health_maintenance.join();
    for handle in worker_handles {
        let _ = handle.join();
    }
}

pub fn run(
    scripts_dir: PathBuf,
    context: crate::node::NodeContext,
    args: NodeServeArgs,
) -> Result<(), Box<dyn Error>> {
    let lifecycle = context.acquire_lifecycle_lock()?;
    context.validate_existing_state_directory()?;
    let initialized = crate::operations::node::initialize_node_locked(
        &context,
        &crate::domain::NodeConfig::default(),
        lifecycle.state_was_present(),
    )?;
    crate::operations::node::recover_local_bootstrap_token_tombstones(
        &context,
        args.bootstrap_token_file.as_deref(),
    )?;
    let configured = initialized
        .status
        .config
        .as_ref()
        .ok_or("node configuration was not initialized")?;
    let node_config = crate::operations::node::load_node_config(&context)?;
    let configured_bind = configured.api_bind.parse()?;
    let api_args = ApiArgs {
        bind: args.bind.unwrap_or(configured_bind),
        allow_non_loopback: args.allow_non_loopback,
        policy: args.policy.clone(),
        tokens_file: args.tokens_file.clone(),
        secret_refs: args.secret_refs.clone(),
    };
    // Fail before bind: policy parse, auth, non-loopback guard.
    let boot = api::prepare_api_boot(&api_args)?;

    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;
    let allow_non_loopback_direct =
        args.allow_non_loopback_direct || boot.deploy.node.allow_non_loopback_direct;
    let direct_bind = resolve_direct_bind(
        args.direct_bind,
        configured.direct_bind.as_deref(),
        allow_non_loopback_direct,
    )?;
    let static_peers = configured.static_peers.clone();
    let workers = args.workers.or(boot.deploy.node.workers).unwrap_or(1);
    let scheduler_enabled = scheduler_enabled(
        args.no_scheduler,
        args.scheduler,
        boot.deploy.node.scheduler,
    );
    // The Performer-side Health Plane reporter. It reads only local facts and
    // only ever reports to a peer the local registry records as an active
    // trusted Conductor; the transport decides nothing about authorization.
    let health_reporter = Arc::new(crate::health_plane::report::HealthReporter::new(Box::new(
        crate::operations::health::NodeHealthFacts::new(
            Workspace::new(workspace.root().to_path_buf()),
            node_config.node.display_name.clone(),
            u64::from(workers),
            scheduler_enabled,
        ),
    )));
    // Before the transport can accept anything. A Cue accepted first and
    // completed fast would otherwise land inside the reporter's own first
    // harvest, which seeds and returns nothing -- the Conductor would wait on
    // an outcome that was never going to be sent.
    health_reporter.seed_run_watermark();
    if let Err(error) = crate::operations::node::reconcile_revoked_cue_runs(&context, &workspace) {
        eprintln!("omakure: revoked Cue cleanup remains pending: {error}");
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    crate::adapters::signals::install_signal_handlers(Arc::clone(&cancel_flag));

    let mut direct_service = if direct_bind.is_some() || !static_peers.is_empty() {
        Some(crate::direct_service::DirectService::start(
            direct_bind,
            &static_peers,
            context.clone(),
            Some(Arc::clone(&health_reporter)),
            Some(workspace.root().to_path_buf()),
        )?)
    } else {
        None
    };

    let discovery_secret = resolve_discovery_secret(&node_config, &workspace)?;
    let mut discovery_service = start_discovery_service(
        node_config.discovery.clone(),
        context.clone(),
        direct_bind.map(|bind| bind.port()),
        discovery_secret,
    )?;

    let readiness_requires_worker =
        args.readiness_requires_worker || boot.deploy.node.readiness_requires_worker;
    let readiness_requires_scheduler =
        args.readiness_requires_scheduler || boot.deploy.node.readiness_requires_scheduler;
    let readiness_requires_transport =
        args.readiness_requires_transport || boot.deploy.node.readiness_requires_transport;

    let readiness = ReadinessGate::new_with_transport(
        readiness_requires_worker,
        readiness_requires_scheduler,
        workers >= 1,
        scheduler_enabled,
        readiness_requires_transport,
        !static_peers.is_empty(),
    );

    let cue_dispatcher = direct_service
        .as_ref()
        .map(|service| service.cue_dispatcher());
    let baseline_dispatcher = direct_service
        .as_ref()
        .map(|service| service.baseline_dispatcher());
    let transport_status = direct_service.as_ref().map(|service| service.status());
    let discovery_status = discovery_service.as_ref().map(|service| service.status());
    let transport_readiness = transport_status.clone();
    let transport_watcher = spawn_transport_watcher(transport_status.as_ref(), &readiness);

    crate::auth::install_sighup_reload(boot.auth.clone());

    let worker_handles = spawn_workers(
        workers,
        &workspace,
        &context,
        &args,
        &readiness,
        &cancel_flag,
    );

    // Health Plane retention. The frozen bounds - 64 Signals per Performer,
    // the 7-day Signal window, the 60-second reorder-buffer lifetime, the
    // replay table, the audit window, and revocation cleanup - are all enforced
    // by the Wave 2 shared operations; this loop only asks them to run on a
    // bounded cadence so a long-lived node stays inside the frozen storage
    // ceiling without operator action.
    let health_maintenance = {
        let context = context.clone();
        let workspace = workspace.clone_for_executor();
        let flag = Arc::clone(&cancel_flag);
        thread::spawn(move || health_maintenance_loop(context, workspace, flag))
    };

    let scheduler_handle = if scheduler_enabled {
        let ws = workspace.clone_for_executor();
        let flag = Arc::clone(&cancel_flag);
        let lifecycle = LoopLifecycle::new(Arc::clone(&readiness), LoopKind::Scheduler, 1);
        Some(thread::spawn(move || {
            run_tracked_loop(lifecycle, || scheduler_loop(ws, flag));
        }))
    } else {
        None
    };
    // Record where this service can be reached, so a CLI process can hand it
    // work that only it can do. Removed on the way out; a stale file is
    // harmless because the client treats a refused connection as "no service".
    let endpoint_path = workspace.service_endpoint_path();
    let _ = std::fs::write(
        &endpoint_path,
        serde_json::json!({ "api_bind": boot.bind.to_string() }).to_string(),
    );
    let _endpoint_guard = ServiceEndpointFile(endpoint_path);

    let health_registry = Arc::new(crate::operations::health::open_observational_registry(
        &context,
    )?);
    let body_limit = boot.deploy.http.body_limit_bytes.max(1);
    let auth_verification_gate = api::auth_verification_gate(&boot.deploy);
    let health_plane = api::health_plane_router(
        Arc::clone(&health_registry),
        boot.auth.clone(),
        boot.deploy.clone(),
        Arc::clone(&auth_verification_gate),
        body_limit,
    );
    let runtime = tokio::runtime::Runtime::new()?;
    let cancel_for_http = Arc::clone(&cancel_flag);
    let readiness_for_http = Arc::clone(&readiness);
    let http_result = runtime.block_on(async move {
        api::serve_http(
            boot.bind,
            boot.auth,
            workspace,
            boot.api_policy,
            boot.deploy,
            Some(readiness_for_http),
            transport_readiness,
            discovery_status,
            cue_dispatcher,
            baseline_dispatcher,
            health_plane,
            args.bootstrap_token_file,
            auth_verification_gate,
            cancel_for_http,
            None,
        )
        .await
    });

    // HTTP stopped (cancel or error). Ensure cancel is set so loops exit, then
    // join scheduler and workers (stop scheduling → stop claiming → drain).
    stop_live_services(
        &cancel_flag,
        transport_watcher,
        &mut direct_service,
        &mut discovery_service,
        &readiness,
    );
    join_background_loops(scheduler_handle, health_maintenance, worker_handles);

    http_result
}

/// Cadence between Health Plane retention passes.
///
/// One minute is the frozen Health Plane rate window and is far shorter than
/// every retention bound it enforces, so no bound can be exceeded between two
/// passes.
const HEALTH_MAINTENANCE_INTERVAL: Duration =
    Duration::from_secs(crate::health_plane::bounds::RATE_MINUTE_WINDOW_SECONDS as u64);

/// Slice between cancellation checks inside one maintenance wait.
const HEALTH_MAINTENANCE_SLICE: Duration = Duration::from_millis(200);

/// Retry run recovery, revoked Cue cleanup, and Health Plane retention on a
/// bounded cadence without stopping the service after an individual failure.
fn health_maintenance_loop(
    context: crate::node::NodeContext,
    workspace: Workspace,
    cancel_flag: Arc<AtomicBool>,
) {
    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            return;
        }
        run_health_maintenance(&context, &workspace);
        let deadline = std::time::Instant::now() + HEALTH_MAINTENANCE_INTERVAL;
        while std::time::Instant::now() < deadline {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(HEALTH_MAINTENANCE_SLICE);
        }
    }
}

fn run_health_maintenance(context: &crate::node::NodeContext, workspace: &Workspace) {
    if let Err(error) = crate::operations::worker::recover_abandoned_remote_runs(workspace) {
        eprintln!("omakure: abandoned Cue recovery remains pending: {error}");
    }
    if let Err(error) = crate::operations::node::reconcile_revoked_cue_runs(context, workspace) {
        eprintln!("omakure: revoked Cue cleanup remains pending: {error}");
    }
    let Ok(identity) = crate::node_identity::NodeIdentity::load_existing(context) else {
        return;
    };
    let Ok(registry) =
        crate::node_registry::NodeRegistry::open_existing(context, identity.public_status())
    else {
        return;
    };
    let plane = crate::health_plane::HealthPlane::new(&registry);
    // Revocation cleanup first: a peer that is no longer actively trusted must
    // stop occupying Health Plane capacity before retention is measured.
    let _ = plane.purge_revoked();
    let _ = plane.prune();
}

fn scheduler_loop(workspace: Workspace, cancel_flag: Arc<AtomicBool>) {
    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            return;
        }
        let tick_start = Utc::now();
        let _ = serve::scheduler_tick(&workspace, tick_start);

        let deadline = std::time::Instant::now() + SCHEDULER_SCAN_INTERVAL;
        while std::time::Instant::now() < deadline {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(SCHEDULER_SCAN_SLICE_MS));
        }
    }
}

/// Removes the service endpoint file when the service stops, however it stops.
struct ServiceEndpointFile(std::path::PathBuf);

impl Drop for ServiceEndpointFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn direct_bind_override_and_loopback_guard_keep_the_same_errors() {
        let loopback: SocketAddr = "127.0.0.1:7879".parse().unwrap();
        let public: SocketAddr = "0.0.0.0:7879".parse().unwrap();
        assert_eq!(resolve_direct_bind(None, None, false).unwrap(), None);
        assert_eq!(
            resolve_direct_bind(Some(loopback), Some("invalid-bind"), false).unwrap(),
            Some(loopback)
        );
        assert_eq!(
            resolve_direct_bind(None, Some("127.0.0.1:7879"), false).unwrap(),
            Some(loopback)
        );
        assert_eq!(
            resolve_direct_bind(Some(public), None, true).unwrap(),
            Some(public)
        );
        assert_eq!(
            resolve_direct_bind(Some(public), None, false)
                .unwrap_err()
                .to_string(),
            "refusing to bind direct transport 0.0.0.0:7879; pass --allow-non-loopback-direct to opt in"
        );
        assert_eq!(
            resolve_direct_bind(None, Some("invalid-bind"), true)
                .unwrap_err()
                .to_string(),
            "invalid socket address syntax"
        );
    }

    #[test]
    fn scheduler_flags_keep_their_precedence() {
        assert!(!scheduler_enabled(true, true, Some(true)));
        assert!(scheduler_enabled(false, true, Some(false)));
        assert!(!scheduler_enabled(false, false, Some(false)));
        assert!(scheduler_enabled(false, false, None));
    }

    #[test]
    fn discovery_secret_is_resolved_only_when_enabled_and_named() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let mut config = crate::domain::NodeConfig::default();
        config.organization.discovery_secret_ref = "invalid-ref".to_string();
        config.discovery.enabled = false;
        assert_eq!(resolve_discovery_secret(&config, &workspace).unwrap(), None);
        config.discovery.enabled = true;
        config.organization.discovery_secret_ref.clear();
        assert_eq!(resolve_discovery_secret(&config, &workspace).unwrap(), None);
        config.organization.discovery_secret_ref = "literal-secret".to_string();
        assert_eq!(
            resolve_discovery_secret(&config, &workspace).unwrap(),
            Some("literal-secret".to_string())
        );
        config.organization.discovery_secret_ref = "secret://".to_string();
        assert_eq!(
            resolve_discovery_secret(&config, &workspace)
                .unwrap_err()
                .to_string(),
            "discovery_secret_invalid"
        );
    }

    #[test]
    fn shutdown_stops_watcher_before_clearing_loop_readiness() {
        let readiness = ReadinessGate::new(true, true, true, true);
        readiness.set_workers_alive(true);
        readiness.set_scheduler_alive(true);
        assert!(readiness.is_ready());
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let watcher_cancel = Arc::new(AtomicBool::new(false));
        let watcher_flag = Arc::clone(&watcher_cancel);
        let watcher_readiness = Arc::clone(&readiness);
        let (observed_tx, observed_rx) = mpsc::channel();
        let watcher = thread::spawn(move || {
            while !watcher_flag.load(Ordering::SeqCst) {
                thread::yield_now();
            }
            observed_tx.send(watcher_readiness.is_ready()).unwrap();
        });
        let mut direct_service = None;
        let mut discovery_service = None;
        stop_live_services(
            &cancel_flag,
            Some((watcher_cancel, watcher)),
            &mut direct_service,
            &mut discovery_service,
            &readiness,
        );
        assert!(cancel_flag.load(Ordering::SeqCst));
        assert!(observed_rx.recv().unwrap());
        assert!(!readiness.is_ready());
    }

    #[test]
    fn readiness_gate_defaults_ready_without_requirements() {
        let gate = ReadinessGate::new(false, false, true, true);
        assert!(gate.is_ready());
    }

    #[test]
    fn readiness_requires_worker_fails_until_alive() {
        let gate = ReadinessGate::new(true, false, true, false);
        assert!(!gate.is_ready());
        gate.set_workers_alive(true);
        assert!(gate.is_ready());
    }

    #[test]
    fn readiness_requires_scheduler_fails_until_alive() {
        let gate = ReadinessGate::new(false, true, false, true);
        assert!(!gate.is_ready());
        gate.set_scheduler_alive(true);
        assert!(gate.is_ready());
    }

    #[test]
    fn readiness_requires_worker_ignored_when_workers_not_configured() {
        let gate = ReadinessGate::new(true, false, false, false);
        assert!(gate.is_ready());
    }

    #[test]
    fn readiness_requires_scheduler_ignored_when_scheduler_disabled() {
        let gate = ReadinessGate::new(false, true, false, false);
        assert!(gate.is_ready());
    }

    #[test]
    fn worker_readiness_tracks_entry_and_unexpected_exit() {
        let gate = ReadinessGate::new(true, false, true, false);
        let lifecycle = LoopLifecycle::new(Arc::clone(&gate), LoopKind::Workers, 2);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let first_lifecycle = Arc::clone(&lifecycle);
        let first = thread::spawn(move || {
            run_tracked_loop(first_lifecycle, || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        entered_rx.recv().unwrap();
        assert!(!gate.is_ready(), "all configured workers must enter first");

        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let (second_release_tx, second_release_rx) = mpsc::channel();
        let second = thread::spawn(move || {
            run_tracked_loop(lifecycle, || {
                second_entered_tx.send(()).unwrap();
                second_release_rx.recv().unwrap();
            });
        });
        second_entered_rx.recv().unwrap();
        assert!(gate.is_ready());

        release_tx.send(()).unwrap();
        first.join().unwrap();
        assert!(
            !gate.is_ready(),
            "one worker exit makes the group unhealthy"
        );
        second_release_tx.send(()).unwrap();
        second.join().unwrap();
    }

    #[test]
    fn scheduler_readiness_clears_when_loop_panics() {
        let gate = ReadinessGate::new(false, true, false, true);
        let lifecycle = LoopLifecycle::new(Arc::clone(&gate), LoopKind::Scheduler, 1);
        let (entered_tx, entered_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            run_tracked_loop(lifecycle, || {
                entered_tx.send(()).unwrap();
                panic!("unexpected scheduler failure");
            });
        });
        entered_rx.recv().unwrap();
        assert!(handle.join().is_err());
        assert!(!gate.is_ready());
    }
}
