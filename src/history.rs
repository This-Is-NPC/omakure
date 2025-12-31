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
    workspace: &Workspace,
    script: &Path,
    args: &[String],
    output: ScriptRunOutput,
) -> HistoryEntry {
    HistoryEntry {
        timestamp: timestamp_ms(),
        script: script_path(workspace, script),
        args: args.to_vec(),
        success: output.success,
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        error: None,
    }
}

pub fn error_entry(
    workspace: &Workspace,
    script: &Path,
    args: &[String],
    message: String,
) -> HistoryEntry {
    HistoryEntry {
        timestamp: timestamp_ms(),
        script: script_path(workspace, script),
        args: args.to_vec(),
        success: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        error: Some(message),
    }
}

pub fn record_entry(workspace: &Workspace, entry: &HistoryEntry) -> io::Result<PathBuf> {
    let data = serde_json::to_vec_pretty(entry)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
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

fn script_path(workspace: &Workspace, script: &Path) -> PathBuf {
    script
        .strip_prefix(workspace.root())
        .unwrap_or(script)
        .to_path_buf()
}

fn timestamp_ms() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_millis() as i64
}
