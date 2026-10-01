use crate::adapters::signals::install_signal_handlers;
use crate::cli::args::QueueWorkerArgs;
use crate::cli::json;
use crate::operations::worker::worker_loop;
use crate::workspace::Workspace;
use serde_json::json;
use std::error::Error;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

pub(super) fn worker(
    workspace: &Workspace,
    opts: QueueWorkerArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    install_signal_handlers(Arc::clone(&cancel_flag));

    let concurrency = opts.concurrency.max(1);
    let mut handles = Vec::with_capacity(concurrency as usize);
    for thread_idx in 0..concurrency {
        let workspace = workspace.clone_for_executor();
        let cancel_flag = Arc::clone(&cancel_flag);
        let actor_filter = opts.actor_filter.clone();
        let script_filter = opts.script_filter.clone();
        let once = opts.once;
        let pid = std::process::id();
        let worker_id = format!("worker:{}-t{}", pid, thread_idx);
        handles.push(thread::spawn(move || {
            worker_loop(
                workspace,
                worker_id,
                cancel_flag,
                actor_filter,
                script_filter,
                once,
            );
        }));
    }
    for h in handles {
        let _ = h.join();
    }

    if json_output {
        json::print_ok(json!({"status": "stopped"}));
    } else {
        println!("worker stopped");
    }
    Ok(())
}
