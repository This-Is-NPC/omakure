use crate::workspace::Workspace;
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) fn log_file(workspace: &Workspace) -> PathBuf {
    workspace.omakure_dir().join("daemon.log")
}

pub(super) fn log_line(path: &Path, level: &str, message: &str) {
    let line = format!("{} [{}] {}\n", Utc::now().to_rfc3339(), level, message);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    } else {
        eprintln!("{line}");
    }
}
