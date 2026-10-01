//! `omakure queue` — push, cancel, drain, and inspect the run queue.
//!
//! The `worker` subcommand is the long-running daemon that drains the
//! queue. Producers (`add`, `cancel`, `dead-letter`, `stats`) operate on
//! the same `runs.sqlite` and are short-lived CLI commands.
//!
//! Both producers and the worker write through the same state machine
//! defined in [`crate::runs`]. The worker shares its execution code path
//! with the synchronous `omakure run` fast path via
//! [`crate::run_executor::execute_with_heartbeat`].

mod producers;

#[cfg(test)]
mod tests;

use crate::cli::args::{QueueArgs, QueueCommand};
use crate::cli::json;
use crate::operations::worker::{run_standalone_workers, StandaloneWorkerOptions};
use crate::workspace::Workspace;
use serde_json::json;
use std::error::Error;
use std::path::PathBuf;

use producers::{add, cancel, dead_letter, stats};

pub fn run(scripts_dir: PathBuf, args: QueueArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;
    match args.command {
        QueueCommand::Add(opts) => add(&workspace, opts, json_output),
        QueueCommand::Cancel(opts) => cancel(&workspace, opts, json_output),
        QueueCommand::DeadLetter(opts) => dead_letter(&workspace, opts, json_output),
        QueueCommand::Worker(opts) => worker(&workspace, opts, json_output),
        QueueCommand::Stats => stats(&workspace, json_output),
    }
}

fn worker(
    workspace: &Workspace,
    opts: crate::cli::args::QueueWorkerArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    run_standalone_workers(
        workspace,
        StandaloneWorkerOptions {
            concurrency: opts.concurrency,
            actor_filter: opts.actor_filter,
            script_filter: opts.script_filter,
            once: opts.once,
        },
    );

    if json_output {
        json::print_ok(json!({"status": "stopped"}));
    } else {
        println!("worker stopped");
    }
    Ok(())
}
