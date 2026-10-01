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
use crate::cli::emit::emit_error;
use crate::cli::json::{self, codes};
use crate::runs::{RunStore, RunsError, TraceLevel};
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
    let mut store = match RunStore::open(&workspace) {
        Ok(store) => store,
        Err(err) => return emit_error(json_output, codes::INTERNAL, err.to_string()),
    };

    let secrets = crate::secrets::secrets_from_env();
    let message = crate::secrets::redact_text(&args.message, &secrets);
    let data = args
        .data
        .as_deref()
        .map(|data| crate::secrets::redact_text(data, &secrets));

    let trace = match store.insert_trace(&run_id, level, &message, data.as_deref()) {
        Ok(trace) => trace,
        Err(RunsError::NotFound(_)) => {
            return emit_error(
                json_output,
                codes::NOT_FOUND,
                format!("run not found: {}", run_id),
            );
        }
        Err(err) => return emit_error(json_output, codes::INTERNAL, err.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::EnqueueOptions;
    use crate::test_support::scratch_workspace;

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
        let ws = scratch_workspace("trace_writes");
        let mut store = RunStore::open(&ws).unwrap();
        let row = store
            .enqueue(
                "/x/a.sh",
                &[],
                EnqueueOptions {
                    actor: "human".into(),
                    omakure_version: "test".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let trace = store
            .insert_trace(&row.run_id, TraceLevel::Info, "hello", Some(r#"{"k":"v"}"#))
            .unwrap();
        assert_eq!(trace.sequence, 1);
        assert_eq!(trace.level, "info");
        let _ = std::fs::remove_dir_all(ws.root());
    }
}
