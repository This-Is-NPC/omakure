use crate::ports::ScriptRunOutput;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: i64,
    pub script: PathBuf,
    pub args: Vec<String>,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

pub fn success_entry(
    _workspace: &Workspace,
    script: &Path,
    args: &[String],
    output: ScriptRunOutput,
) -> HistoryEntry {
    HistoryEntry {
        timestamp: timestamp_ms(),
        script: script_path(script),
        args: args.to_vec(),
        success: output.success,
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        error: None,
    }
}

pub fn error_entry(
    _workspace: &Workspace,
    script: &Path,
    args: &[String],
    message: String,
) -> HistoryEntry {
    HistoryEntry {
        timestamp: timestamp_ms(),
        script: script_path(script),
        args: args.to_vec(),
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        error: Some(message),
    }
}

pub fn record_entry(workspace: &Workspace, entry: &HistoryEntry) -> io::Result<PathBuf> {
    let data = serde_json::to_vec_pretty(entry).map_err(io::Error::other)?;
    let file_name = history_file_name(entry);
    let path = workspace.history_dir().join(file_name);
    fs::write(&path, data)?;
    Ok(path)
}

pub fn load_entries(workspace: &Workspace) -> io::Result<Vec<HistoryEntry>> {
    let mut entries = Vec::new();
    let dir_entries = match fs::read_dir(workspace.history_dir()) {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                return Ok(entries);
            }
            return Err(err);
        }
    };

    for entry in dir_entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        let parsed: HistoryEntry = match serde_json::from_slice(&data) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        entries.push(parsed);
    }

    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(entries)
}

pub fn format_output(entry: &HistoryEntry) -> String {
    if let Some(error) = &entry.error {
        return error.trim().to_string();
    }
    let mut parts = Vec::new();
    if !entry.stdout.trim().is_empty() {
        parts.push(format!("STDOUT:\n{}", entry.stdout.trim_end()));
    }
    if !entry.stderr.trim().is_empty() {
        parts.push(format!("STDERR:\n{}", entry.stderr.trim_end()));
    }
    parts.join("\n\n")
}

pub fn format_timestamp(timestamp_ms: i64) -> String {
    let mut ms = timestamp_ms;
    if ms < 0 {
        ms = 0;
    }
    let seconds = ms / 1000;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;

    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        year, month, day, hour, minute
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn history_file_name(entry: &HistoryEntry) -> String {
    let slug = safe_slug(&entry.script.to_string_lossy());
    format!("{}-{}-{}.json", entry.timestamp, std::process::id(), slug)
}

fn safe_slug(input: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_');
    let mut slug = trimmed.to_string();
    if slug.is_empty() {
        slug = "run".to_string();
    }
    const LIMIT: usize = 64;
    if slug.len() > LIMIT {
        slug.truncate(LIMIT);
    }
    slug
}

/// Resolve the path that should be persisted in a history entry.
///
/// History entries are keyed by the **absolute canonical** path of the
/// executed script so that the same physical script always produces the
/// same key, regardless of which working directory or scripts root the
/// run was launched from. When canonicalization fails (e.g. the script
/// no longer exists by the time the entry is recorded), fall back to the
/// path as supplied — better an addressable string than a panic.
fn script_path(script: &Path) -> PathBuf {
    fs::canonicalize(script).unwrap_or_else(|_| script.to_path_buf())
}

fn timestamp_ms() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        // 2024-01-15 12:30 UTC = 1705321800000 ms
        let ts = 1705321800000_i64;
        let formatted = format_timestamp(ts);
        assert_eq!(formatted, "2024-01-15 12:30");
    }

    #[test]
    fn test_format_timestamp_zero() {
        let formatted = format_timestamp(0);
        assert_eq!(formatted, "1970-01-01 00:00");
    }

    #[test]
    fn test_format_timestamp_negative() {
        let formatted = format_timestamp(-1000);
        assert_eq!(formatted, "1970-01-01 00:00");
    }

    #[test]
    fn test_safe_slug_simple() {
        assert_eq!(safe_slug("hello"), "hello");
        assert_eq!(safe_slug("Hello World"), "hello_world");
    }

    #[test]
    fn test_safe_slug_special_chars() {
        assert_eq!(safe_slug("my-script.bash"), "my_script_bash");
        assert_eq!(safe_slug("path/to/script"), "path_to_script");
    }

    #[test]
    fn test_safe_slug_consecutive_special() {
        assert_eq!(safe_slug("a--b__c"), "a_b_c");
    }

    #[test]
    fn test_safe_slug_empty() {
        assert_eq!(safe_slug(""), "run");
        assert_eq!(safe_slug("---"), "run");
    }

    #[test]
    fn test_safe_slug_truncation() {
        let long_name = "a".repeat(100);
        let slug = safe_slug(&long_name);
        assert!(slug.len() <= 64);
    }

    #[test]
    fn test_format_output_success() {
        let entry = HistoryEntry {
            timestamp: 0,
            script: PathBuf::from("test.bash"),
            args: vec![],
            success: true,
            exit_code: Some(0),
            stdout: "output here\n".to_string(),
            stderr: "".to_string(),
            error: None,
        };
        let output = format_output(&entry);
        assert!(output.contains("STDOUT:"));
        assert!(output.contains("output here"));
    }

    #[test]
    fn script_path_returns_absolute_canonical_for_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "omakure_history_script_path_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let script_file = dir.join("run.sh");
        fs::write(&script_file, "#!/bin/bash\n").expect("write script");

        // Pass the file via a relative path to confirm canonicalization.
        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&dir).expect("chdir tmp");
        let resolved = script_path(&PathBuf::from("run.sh"));
        std::env::set_current_dir(&prev).expect("restore cwd");

        assert!(resolved.is_absolute(), "expected absolute path: {:?}", resolved);
        let canonical = fs::canonicalize(&script_file).expect("canonicalize");
        assert_eq!(resolved, canonical);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn script_path_falls_back_to_input_when_canonicalization_fails() {
        let missing = PathBuf::from("/__omakure_definitely_missing_script__");
        let resolved = script_path(&missing);
        assert_eq!(resolved, missing);
    }

    #[test]
    fn record_and_load_round_trip_preserves_absolute_script_path() {
        use crate::ports::ScriptRunOutput;
        use crate::workspace::Workspace;

        let dir = std::env::temp_dir().join(format!(
            "omakure_history_round_trip_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let global = dir.join("global");
        let scripts = dir.join("scripts");
        fs::create_dir_all(&scripts).expect("create scripts");

        let script_file = scripts.join("run.sh");
        fs::write(&script_file, "#!/bin/bash\n").expect("write script");

        let workspace = Workspace::with_scripts_root(global.clone(), scripts.clone(), true);
        workspace.ensure_layout().expect("layout");

        let output = ScriptRunOutput {
            stdout: "ok".into(),
            stderr: String::new(),
            exit_code: Some(0),
            success: true,
        };
        let entry = success_entry(&workspace, &script_file, &[], output);
        record_entry(&workspace, &entry).expect("record");

        let loaded = load_entries(&workspace).expect("load");
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].script.is_absolute());
        let canonical = fs::canonicalize(&script_file).expect("canonicalize");
        assert_eq!(loaded[0].script, canonical);

        // Confirm absolutely no metadata was created in the scripts root.
        assert!(!scripts.join(".history").exists());
        assert!(!scripts.join(".omaken").exists());
        assert!(!scripts.join("omakure.toml").exists());

        // History was written to the global root.
        assert!(global.join(".history").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_relative_history_entries_still_load() {
        // Insert a legacy-shaped entry directly into .history/ as JSON
        // and confirm load_entries returns it without dropping or
        // erroring on the relative `script` field.
        use crate::workspace::Workspace;

        let dir = std::env::temp_dir().join(format!(
            "omakure_history_legacy_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let workspace = Workspace::new(dir.clone());
        workspace.ensure_layout().expect("layout");

        let legacy_json = r#"{
            "timestamp": 1700000000000,
            "script": "team/run.sh",
            "args": [],
            "success": true,
            "exit_code": 0,
            "stdout": "",
            "stderr": "",
            "error": null
        }"#;
        fs::write(workspace.history_dir().join("1700000000000-1-legacy.json"), legacy_json)
            .expect("write legacy entry");

        let loaded = load_entries(&workspace).expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].script, PathBuf::from("team/run.sh"));
        assert!(!loaded[0].script.is_absolute());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_format_output_with_error() {
        let entry = HistoryEntry {
            timestamp: 0,
            script: PathBuf::from("test.bash"),
            args: vec![],
            success: false,
            exit_code: None,
            stdout: "".to_string(),
            stderr: "".to_string(),
            error: Some("Script failed to run".to_string()),
        };
        let output = format_output(&entry);
        assert_eq!(output, "Script failed to run");
    }
}
