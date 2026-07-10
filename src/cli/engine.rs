//! `omakure engine` — HTTP API + optional in-process workers + scheduler.
//!
//! Composes existing `api::serve_http`, `queue::worker_loop`, and
//! `serve::scheduler_tick` under one cancel flag. Shutdown order:
//! stop accepting HTTP → stop scheduling → stop claiming → drain/join workers.

use crate::cli::api::{self, ReadinessGate};
use crate::cli::args::{ApiArgs, EngineArgs};
use crate::cli::queue;
use crate::cli::serve;
use crate::workspace::Workspace;
use chrono::Utc;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const SCHEDULER_SCAN_SLICE_MS: u64 = 200;
const SCHEDULER_SCAN_INTERVAL: Duration = Duration::from_secs(5);

pub fn run(scripts_dir: PathBuf, args: EngineArgs) -> Result<(), Box<dyn Error>> {
    let api_args = ApiArgs {
        bind: args.bind,
        allow_non_loopback: args.allow_non_loopback,
        policy: args.policy.clone(),
        tokens_file: args.tokens_file.clone(),
        capabilities: args.capabilities.clone(),
        secret_refs: args.secret_refs.clone(),
    };
    // Fail before bind: policy parse, auth, non-loopback guard.
    let boot = api::prepare_api_boot(&api_args)?;

    let workers = args
        .workers
        .or(boot.deploy.engine.workers)
        .unwrap_or(1);
    let scheduler_enabled = if args.no_scheduler {
        false
    } else if args.scheduler {
        true
    } else {
        boot.deploy.engine.scheduler.unwrap_or(true)
    };
    let readiness_requires_worker =
        args.readiness_requires_worker || boot.deploy.engine.readiness_requires_worker;
    let readiness_requires_scheduler =
        args.readiness_requires_scheduler || boot.deploy.engine.readiness_requires_scheduler;

    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let readiness = ReadinessGate::new(
        readiness_requires_worker,
        readiness_requires_scheduler,
        workers >= 1,
        scheduler_enabled,
    );

    let cancel_flag = Arc::new(AtomicBool::new(false));
    queue::install_signal_handlers(Arc::clone(&cancel_flag));
    if boot.auth.is_file_mode() {
        crate::auth::install_sighup_reload(boot.auth.clone());
    }

    let mut worker_handles = Vec::new();
    if workers >= 1 {
        for thread_idx in 0..workers {
            let ws = workspace.clone_for_executor();
            let flag = Arc::clone(&cancel_flag);
            let actor_filter = args.worker_actor_filter.clone();
            let script_filter = args.worker_script_filter.clone();
            let worker_id = format!("engine-worker:{}-t{}", std::process::id(), thread_idx);
            worker_handles.push(thread::spawn(move || {
                queue::worker_loop(ws, worker_id, flag, actor_filter, script_filter, false);
            }));
        }
        readiness.set_workers_alive(true);
    }

    let scheduler_handle = if scheduler_enabled {
        let ws = workspace.clone_for_executor();
        let flag = Arc::clone(&cancel_flag);
        Some(thread::spawn(move || {
            scheduler_loop(ws, flag);
        }))
    } else {
        None
    };
    if scheduler_enabled {
        // Mark alive on the supervisor thread (same as workers) so
        // `--readiness-requires-scheduler` cannot race HTTP startup.
        readiness.set_scheduler_alive(true);
    }

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
            cancel_for_http,
            None,
        )
        .await
    });

    // HTTP stopped (cancel or error). Ensure cancel is set so loops exit, then
    // join scheduler and workers (stop scheduling → stop claiming → drain).
    cancel_flag.store(true, Ordering::SeqCst);
    readiness.set_workers_alive(false);
    readiness.set_scheduler_alive(false);

    if let Some(h) = scheduler_handle {
        let _ = h.join();
    }
    for h in worker_handles {
        let _ = h.join();
    }

    http_result
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
