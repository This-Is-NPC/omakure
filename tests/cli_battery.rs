use std::path::PathBuf;
use std::process::Command;

fn omakure_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omakure"))
}

fn unique_temp(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("omakure_battery_test_{label}_{pid}_{nanos}"))
}

#[test]
fn json_battery_errors_emit_single_stdout_envelope_without_stderr() {
    let dir = unique_temp("json_error");
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let output = Command::new(omakure_bin())
        .arg("--scripts-dir")
        .arg(&dir)
        .arg("--json")
        .arg("battery")
        .arg("inspect")
        .arg("missing")
        .output()
        .expect("spawn omakure");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(!output.status.success(), "expected non-zero exit");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "expected one JSON line, got: {stdout}");
    let envelope: serde_json::Value = serde_json::from_str(lines[0]).expect("json envelope");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "not_found");
}
