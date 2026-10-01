use serde_json::Value;
use std::process::Output;

pub fn output_text(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn json_output(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "expected successful command: {}",
        output_text(output)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON output ({error}): {}", output_text(output)))
}
