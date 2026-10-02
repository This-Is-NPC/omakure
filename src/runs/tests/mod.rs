use super::ids::generate_run_id;
use super::lifecycle::dead_letter;
use super::open::{init_schema, open_connection, runs_db_path};
use super::query::{get_run_required, has_live_scheduled_run, query_runs, stats};
use super::*;
use crate::test_support::scratch_workspace;
use crate::util::time::unix_millis;
use rusqlite::{Connection, params};
use std::fs;
use std::time::Duration;

mod enqueue;
mod ids;
mod lifecycle;
mod open;
mod query;
mod state;
mod trace;

fn enqueue_opts() -> EnqueueOptions {
    EnqueueOptions {
        actor: "human".into(),
        omakure_version: "test".into(),
        ..Default::default()
    }
}

fn ok_completion() -> RunCompletion {
    RunCompletion {
        stdout: "out".into(),
        stderr: "".into(),
        exit_code: Some(0),
        success: true,
        error: None,
    }
}

fn fail_completion() -> RunCompletion {
    RunCompletion {
        stdout: "".into(),
        stderr: "boom".into(),
        exit_code: Some(2),
        success: false,
        error: None,
    }
}
