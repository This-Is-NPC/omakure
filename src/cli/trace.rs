//! `omakure trace` — script-side trace writer.
//!
//! Designed to be called from inside a script that was launched by
//! `omakure run` or `omakure queue worker`. Both inject `OMAKURE_RUN_ID`
//! into the child environment so this verb knows which run to attach
//! traces to.
//!
//! Outside that context (script run standalone, or copy-pasted into a
//! shell), `OMAKURE_RUN_ID` is unset and the verb becomes a silent
//! no-op so scripts remain testable in isolation.

use crate::cli::args::TraceArgs;
use crate::cli::json::{self, codes};
use crate::runs::{self, TraceLevel};
use crate::workspace::Workspace;
use serde_json::json;
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::str::FromStr;

pub fn run(scripts_dir: PathBuf, args: TraceArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    // No run id, no trace. Print a single warning to stderr (not stdout
    // — agents read stdout) and exit 0 so the calling script keeps going.
    let run_id = match env::var("OMAKURE_RUN_ID") {
        Ok(id) => id,
        Err(_) => {
            eprintln!("omakure trace: OMAKURE_RUN_ID not set, ignoring");
            return Ok(());
        }
    };

    let level = match TraceLevel::from_str(&args.level) {
        Ok(level) => level,
        Err(err) => {
            return emit_error(json_output, codes::INVALID_ARGUMENT, err);
        }
    };

    if let Some(data) = args.data.as_deref() {
        if let Err(err) = serde_json::from_str::<serde_json::Value>(data) {
            return emit_error(
                json_output,
                codes::INVALID_ARGUMENT,
                format!("--data is not valid JSON: {}", err),
            );
        }
    }

    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;
    let mut conn = match runs::open(&workspace) {
        Ok(c) => c,
        Err(err) => return emit_error(json_output, codes::INTERNAL, err),
    };

    let trace = match runs::insert_trace(
        &mut conn,
        &run_id,
        level,
        &args.message,
        args.data.as_deref(),
    ) {
        Ok(trace) => trace,
        Err(err) if err.starts_with("not_found") => {
            return emit_error(
                json_output,
                codes::NOT_FOUND,
                format!("run not found: {}", run_id),
            );
        }
        Err(err) => return emit_error(json_output, codes::INTERNAL, err),
    };

    if json_output {
        json::print_ok(json!({
            "trace_id": trace.trace_id,
            "run_id": trace.run_id,
            "sequence": trace.sequence,
            "level": trace.level,
            "timestamp": trace.timestamp,
        }));
    }
    Ok(())
}

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{enqueue, EnqueueOptions};

    fn make_workspace(label: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!(
            "omakure_trace_test_{}_{}_{}",
            label,
            std::process::id(),
            runs::current_unix_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(dir);
        ws.ensure_layout().unwrap();
        ws
    }

    #[test]
    fn invalid_level_rejected_at_validation() {
        // Test the level parsing in isolation since the full run() path
        // touches stdout / process::exit. We exercise the same FromStr
        // helper used by the dispatch.
        assert!(TraceLevel::from_str("critical").is_err());
        assert!(TraceLevel::from_str("info").is_ok());
    }

    #[test]
    fn insert_trace_writes_row() {
        let ws = make_workspace("trace_writes");
        let mut conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            "/x/a.sh",
            &[],
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let trace = runs::insert_trace(
            &mut conn,
            &row.run_id,
            TraceLevel::Info,
            "hello",
            Some(r#"{"k":"v"}"#),
        )
        .unwrap();
        assert_eq!(trace.sequence, 1);
        assert_eq!(trace.level, "info");
        let _ = std::fs::remove_dir_all(ws.root());
    }
}
