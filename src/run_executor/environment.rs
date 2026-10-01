use crate::runtime::bash_safe_path;
use crate::workspace::Workspace;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

pub(super) struct RedactionFile {
    pub(super) path: PathBuf,
}

impl Drop for RedactionFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn write_redaction_file(
    workspace: &Workspace,
    run_id: &str,
    secrets: &[String],
) -> Result<Option<RedactionFile>, String> {
    let Some(value) = crate::secrets::secrets_env_value(secrets) else {
        return Ok(None);
    };
    fs::create_dir_all(workspace.history_dir())
        .map_err(|err| format!("create redaction dir failed: {err}"))?;
    let path = workspace.history_dir().join(format!(
        ".redact.{}.{}.tmp",
        sanitize_run_id_for_filename(run_id),
        crate::util::time::unix_millis()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&path)
        .map_err(|err| format!("open redaction file failed: {err}"))?;
    file.write_all(value.as_bytes())
        .map_err(|err| format!("write redaction file failed: {err}"))?;
    Ok(Some(RedactionFile { path }))
}

fn sanitize_run_id_for_filename(run_id: &str) -> String {
    run_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

/// Path to the running omakure binary, normalized for bash scripts on Windows.
pub(super) fn push_reserved_run_env(
    env: &mut Vec<(String, String)>,
    workspace: &Workspace,
    run_id: &str,
) {
    env.push(("OMAKURE_RUN_ID".to_string(), run_id.to_string()));
    env.push((
        "OMAKURE_SCRIPTS_DIR".to_string(),
        bash_safe_path(workspace.root()),
    ));
    if let Some(bin) = bash_safe_current_exe() {
        env.push(("OMAKURE_BIN".to_string(), bin));
    }
}

pub(super) fn bash_safe_current_exe() -> Option<String> {
    std::env::current_exe().ok().map(|exe| bash_safe_path(&exe))
}
