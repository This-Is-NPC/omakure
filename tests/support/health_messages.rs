use crate::health_ids::hex16;
use serde_json::{Value, json};

pub fn signal_payload(
    target: &str,
    message_seed: u64,
    sequence: u64,
    signal_seed: u64,
    occurred_at: i64,
) -> Value {
    json!({
        "health_version": 1,
        "message_id": hex16(message_seed),
        "signal": {
            "kind": "run-completed",
            "occurred_at": occurred_at,
            "run": {
                "exit_code": 0,
                "finished_at": occurred_at,
                "run_id": hex16(signal_seed + 900_000),
                "script": "deploy",
                "state": "completed"
            },
            "sequence": sequence,
            "signal_id": hex16(signal_seed),
            "subject": Value::Null
        },
        "target": target,
    })
}
