use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::files::replace_file_atomically;
use super::git_url::{redacted_git_url, url_contains_credentials};
use super::path_safety::reject_symlink_components;
use super::sync::resolve_battery_token;
use super::types::{BatteryAuth, BatteryAuthMethod};
use crate::adapters::git::{self as git_adapter, GitProbeError, GitProcess};
use crate::secrets::SecretAccess;
use crate::workspace::Workspace;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;

pub(super) struct GitAskpassGuard {
    _temp: tempfile::TempDir,
    pub(super) script_path: PathBuf,
    pub(super) token: String,
}

pub(super) struct GitExecContext<'a> {
    pub(super) policy: GitTransportPolicy,
    pub(super) askpass: Option<&'a GitAskpassGuard>,
    pub(super) http_pin: Option<&'a GitHttpPin>,
    pub(super) global_config: Option<&'a Path>,
}

pub(super) fn prepare_git_config(workspace: &Workspace) -> OperationResult<PathBuf> {
    prepare_git_config_in(workspace.omakure_dir())
}

pub(super) fn prepare_git_config_in(dir: &Path) -> OperationResult<PathBuf> {
    fs::create_dir_all(dir).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create git config directory: {err}"),
        )
    })?;
    let path = dir.join("git-empty-config");
    replace_file_atomically(&path, b"", "git isolation config")?;
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitHttpPin {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) address: std::net::IpAddr,
    pub(super) credential_authority: String,
}

impl GitHttpPin {
    pub(super) fn curlopt_resolve(&self) -> String {
        // curl's `HOST:PORT:ADDRESS` --resolve syntax needs `[HOST]` whenever
        // HOST itself is an IPv6 literal, or the colons make it ambiguous
        // with the PORT/ADDRESS delimiters.
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let address = match self.address {
            std::net::IpAddr::V4(address) => address.to_string(),
            std::net::IpAddr::V6(address) => format!("[{address}]"),
        };
        format!("{host}:{}:{address}", self.port)
    }

    pub(super) fn credential_authority(&self) -> &str {
        &self.credential_authority
    }
}

pub(super) fn prepare_git_askpass(
    workspace: &Workspace,
    auth: Option<&BatteryAuth>,
    access: &SecretAccess,
) -> OperationResult<Option<GitAskpassGuard>> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    if !matches!(auth.method, BatteryAuthMethod::HttpsTokenRef) {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "unsupported battery auth method",
        ));
    }
    let token = resolve_battery_token(workspace, &auth.token_ref, access)?;
    if token.is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "battery token_ref resolved to an empty secret",
        ));
    }
    // Unique per-sync directory on tmpfs (/dev/shm on Linux), never on workspace overlay.
    let temp = crate::util::exec::generated_executable_tempdir().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create askpass temp directory: {err}"),
        )
    })?;
    let dir = temp.path().to_path_buf();
    let token_path = dir.join("token");
    write_secret_file(&token_path, token.as_bytes(), 0o600)?;
    let script_path = dir.join("askpass.sh");
    // Resolve token via relative path under $0's directory — no shell-quoted
    // absolute paths (avoids `'` injection and path-with-spaces breakage).
    let script = "#!/bin/sh\nDIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\n[ -n \"$OMAKURE_GIT_AUTHORITY\" ] || exit 1\ncase \"$1\" in\n*\"//$OMAKURE_GIT_AUTHORITY/\"*|*\"//$OMAKURE_GIT_AUTHORITY'\"*|*\"@$OMAKURE_GIT_AUTHORITY/\"*|*\"@$OMAKURE_GIT_AUTHORITY'\"*) ;;\n*) exit 1 ;;\nesac\ncase \"$1\" in\n*Username*|*username*) printf '%s\\n' 'x-access-token' ;;\n*) cat \"$DIR/token\" ;;\nesac\n";
    let script_temp = dir.join(format!(".askpass.sh.{}.tmp", std::process::id()));
    write_secret_file(&script_temp, script.as_bytes(), 0o700)?;
    fs::rename(&script_temp, &script_path).map_err(|err| {
        let _ = fs::remove_file(&script_temp);
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to install askpass script: {err}"),
        )
    })?;
    // Optional: fsync parent directory so rename is durable; errors are non-fatal.
    if let Ok(parent) = File::open(&dir) {
        let _ = parent.sync_all();
    }
    Ok(Some(GitAskpassGuard {
        _temp: temp,
        script_path,
        token,
    }))
}

fn write_secret_file(path: &Path, contents: &[u8], mode: u32) -> OperationResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create secret file: {err}"),
        )
    })?;
    file.write_all(contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to write secret file: {err}"),
        )
    })?;
    file.sync_all().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to sync secret file: {err}"),
        )
    })?;
    // Mode is set at create via OpenOptionsExt::mode; do not chmod after close.
    // Post-close set_permissions on a path about to be exec'd can race ETXTBSY on Linux/musl.
    drop(file);
    Ok(())
}

pub(super) fn run_git_with_context(
    spec: GitCommandSpec,
    ctx: &GitExecContext<'_>,
) -> OperationResult<()> {
    let pin = ctx.http_pin.map(GitHttpPin::curlopt_resolve);
    let output = git_adapter::run(&git_process(&spec, ctx, pin.as_deref())).map_err(|err| {
        OperationError::new(
            OperationErrorCode::GitFailed,
            format!("failed to spawn git: {err}"),
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            sanitize_git_output(
                &String::from_utf8_lossy(&output.stderr),
                ctx.askpass.map(|a| a.token.as_str()),
            ),
        ))
    }
}

pub(super) fn run_git_capture(spec: GitCommandSpec) -> OperationResult<String> {
    run_git_capture_with_policy(spec, GitTransportPolicy::Default)
}

fn run_git_capture_with_policy(
    spec: GitCommandSpec,
    policy: GitTransportPolicy,
) -> OperationResult<String> {
    let cache_path = spec
        .args
        .windows(2)
        .find(|pair| pair[0] == "-C")
        .map(|pair| PathBuf::from(&pair[1]))
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::GitFailed,
                "git command is missing an isolated working directory",
            )
        })?;
    let git_config = match cache_path.ancestors().find(|path| {
        path.file_name()
            .is_some_and(|name| name == crate::workspace::METADATA_DIR)
    }) {
        Some(dir) => prepare_git_config_in(dir)?,
        None => PathBuf::from(".omakure/git-empty-config"),
    };
    run_git_capture_with_context(
        spec,
        &GitExecContext {
            policy,
            askpass: None,
            http_pin: None,
            global_config: Some(&git_config),
        },
    )
}

pub(super) fn run_git_capture_with_context(
    spec: GitCommandSpec,
    ctx: &GitExecContext<'_>,
) -> OperationResult<String> {
    let pin = ctx.http_pin.map(GitHttpPin::curlopt_resolve);
    let output = git_adapter::run(&git_process(&spec, ctx, pin.as_deref())).map_err(|err| {
        OperationError::new(
            OperationErrorCode::GitFailed,
            format!("failed to spawn git: {err}"),
        )
    })?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(redact_token_in_text(
            &stdout,
            ctx.askpass.map(|a| a.token.as_str()),
        ))
    } else {
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            sanitize_git_output(
                &String::from_utf8_lossy(&output.stderr),
                ctx.askpass.map(|a| a.token.as_str()),
            ),
        ))
    }
}

/// Whether the installed `git` supports `http.curloptResolve`. This depends
/// only on the installed git binary, not on any per-sync state, so the
/// process-wide conclusive result is cached instead of spawning
/// `git help --config` on every battery sync. Transient execution failures are
/// not cached so a later sync can recover without restarting the process. The
/// mutex is held across the probe so concurrent callers single-flight onto
/// one subprocess spawn instead of a thundering herd. The lock is recovered
/// on poisoning rather than propagating the panic, since a poisoned lock here
/// would otherwise wedge every future battery sync in the process for good.
static GIT_HTTP_PINNING_SUPPORTED: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

/// Ceiling on the probe subprocess so a hung `git` can't stall the
/// single-flight lock (and therefore every queued battery sync) forever.
const GIT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Like `run_git_capture_with_context`, but kills the child and returns an
/// error if it doesn't exit within `timeout` instead of blocking forever.
/// Only safe for commands with small, bounded output (like `git help
/// --config`): stdout/stderr are read after exit, not streamed, so a command
/// that fills the OS pipe buffer before exiting could still deadlock.
fn run_git_capture_with_timeout(
    spec: GitCommandSpec,
    ctx: &GitExecContext<'_>,
    timeout: std::time::Duration,
) -> OperationResult<String> {
    let pin = ctx.http_pin.map(GitHttpPin::curlopt_resolve);
    let output = git_adapter::run_with_timeout(&git_process(&spec, ctx, pin.as_deref()), timeout)
        .map_err(|error| {
        let message = match error {
            GitProbeError::Spawn(err) => format!("failed to spawn git: {err}"),
            GitProbeError::Wait(err) => format!("failed to wait for git: {err}"),
            GitProbeError::Timeout(timeout) => format!("git probe timed out after {timeout:?}"),
        };
        OperationError::new(OperationErrorCode::GitFailed, message)
    })?;
    if output.status.success() {
        Ok(redact_token_in_text(
            &output.stdout,
            ctx.askpass.map(|a| a.token.as_str()),
        ))
    } else {
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            sanitize_git_output(
                &String::from_utf8_lossy(&output.stderr),
                ctx.askpass.map(|a| a.token.as_str()),
            ),
        ))
    }
}

pub(super) fn assert_git_http_pinning_supported(ctx: &GitExecContext<'_>) -> OperationResult<()> {
    assert_git_http_pinning_supported_with(&GIT_HTTP_PINNING_SUPPORTED, || {
        run_git_capture_with_timeout(
            GitCommandSpec {
                program: "git".into(),
                args: vec!["--no-pager".into(), "help".into(), "--config".into()],
            },
            ctx,
            GIT_PROBE_TIMEOUT,
        )
    })
}

pub(super) fn assert_git_http_pinning_supported_with<F>(
    cache: &std::sync::Mutex<Option<bool>>,
    probe: F,
) -> OperationResult<()>
where
    F: FnOnce() -> OperationResult<String>,
{
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let supported = match *guard {
        Some(supported) => supported,
        None => {
            let output = probe()?;
            let detected = output.lines().any(|key| key == "http.curloptResolve");
            *guard = Some(detected);
            detected
        }
    };
    drop(guard);
    if supported {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            "installed git does not support http.curloptResolve; refusing unpinned battery fetch",
        ))
    }
}

#[cfg(test)]
pub(super) fn git_command(spec: &GitCommandSpec, policy: GitTransportPolicy) -> Command {
    git_command_with_context(
        spec,
        &GitExecContext {
            policy,
            askpass: None,
            http_pin: None,
            global_config: Some(Path::new(".omakure/git-empty-config")),
        },
    )
}

#[cfg(test)]
pub(super) fn git_command_with_context(spec: &GitCommandSpec, ctx: &GitExecContext<'_>) -> Command {
    let pin = ctx.http_pin.map(GitHttpPin::curlopt_resolve);
    git_adapter::command(&git_process(spec, ctx, pin.as_deref()))
}

fn git_process<'a>(
    spec: &'a GitCommandSpec,
    ctx: &'a GitExecContext<'_>,
    pin: Option<&'a str>,
) -> GitProcess<'a> {
    GitProcess {
        program: &spec.program,
        args: &spec.args,
        allowed_protocols: ctx.policy.allowed_protocols(),
        global_config: ctx.global_config,
        askpass: ctx.askpass.map(|guard| guard.script_path.as_path()),
        credential_authority: ctx.http_pin.map(GitHttpPin::credential_authority),
        curlopt_resolve: pin,
    }
}

#[cfg(test)]
pub(super) fn sanitize_git_stderr(stderr: &str) -> String {
    sanitize_git_output(stderr, None)
}

pub(super) fn sanitize_git_output(stderr: &str, token: Option<&str>) -> String {
    let mut message = redact_token_in_text(stderr.trim(), token);
    for part in stderr.split_whitespace() {
        if url_contains_credentials(part) {
            message = message.replace(part, &redacted_git_url(part));
        }
    }
    if message.is_empty() {
        "git command failed".to_string()
    } else {
        message
    }
}

fn redact_token_in_text(text: &str, token: Option<&str>) -> String {
    match token {
        Some(token) if !token.is_empty() && text.contains(token) => {
            text.replace(token, "<redacted>")
        }
        _ => text.to_string(),
    }
}

pub(super) fn reject_unsafe_local_git_config(cache_path: &Path) -> OperationResult<()> {
    let config_path = cache_path.join(".git/config");
    reject_symlink_components(cache_path, Path::new(".git/config"), true)?;
    let config = fs::read_to_string(&config_path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to read local git config: {err}"),
        )
    })?;
    reject_unsafe_git_config_text(&config)?;
    let worktree_config = cache_path.join(".git/config.worktree");
    if worktree_config.exists() {
        reject_symlink_components(cache_path, Path::new(".git/config.worktree"), true)?;
        let config = fs::read_to_string(&worktree_config).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to read local worktree git config: {err}"),
            )
        })?;
        reject_unsafe_git_config_text(&config)?;
        return Err(OperationError::new(
            OperationErrorCode::Conflict,
            "battery cache uses local worktree git config",
        ));
    }
    Ok(())
}

pub(super) fn reject_unsafe_git_config_text(config: &str) -> OperationResult<()> {
    let mut section = String::new();
    for raw_line in config.lines() {
        if let Some(entry) = unsafe_git_config_entry(&mut section, raw_line) {
            return Err(OperationError::new(
                OperationErrorCode::Conflict,
                format!("battery cache has unsafe local git config: {entry}"),
            ));
        }
    }
    Ok(())
}

fn unsafe_git_config_entry(section: &mut String, raw_line: &str) -> Option<String> {
    let line = raw_line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
        return None;
    }
    if line.starts_with('[') && line.ends_with(']') {
        *section = line[1..line.len() - 1].trim().to_ascii_lowercase();
        return unsafe_git_config_section(section).then(|| section.clone());
    }
    let key = line
        .split_once('=')
        .map(|(key, _)| key)
        .unwrap_or(line)
        .trim()
        .to_ascii_lowercase();
    unsafe_git_config_key(section, &key).then(|| format!("{section}.{key}"))
}

fn unsafe_git_config_section(section: &str) -> bool {
    matches!(section, "include" | "http")
        || section.starts_with("includeif ")
        || section.starts_with("includeif.")
        || section.starts_with("http ")
}

fn unsafe_git_config_key(section: &str, key: &str) -> bool {
    match key {
        "helper" => section == "credential" || section.starts_with("credential "),
        "askpass" | "sshcommand" | "worktree" => section == "core",
        "worktreeconfig" => section == "extensions",
        "insteadof" => section.starts_with("url "),
        "proxy" | "proxyauthmethod" => section.starts_with("remote "),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GitTransportPolicy {
    Default,
    HttpsOnly,
}

impl GitTransportPolicy {
    fn allowed_protocols(self) -> &'static str {
        match self {
            Self::Default => "file:https:http",
            Self::HttpsOnly => "https",
        }
    }
}

pub fn git_clone_spec(git_url: &str, cache_path: &Path) -> GitCommandSpec {
    GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "protocol.ext.allow=never".into(),
            "-c".into(),
            "credential.helper=".into(),
            "clone".into(),
            "--no-recurse-submodules".into(),
            "--".into(),
            git_url.into(),
            cache_path.display().to_string(),
        ],
    }
}

pub fn git_fetch_spec(cache_path: &Path, requested_ref: &str) -> GitCommandSpec {
    GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-C".into(),
            cache_path.display().to_string(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "protocol.ext.allow=never".into(),
            "-c".into(),
            "credential.helper=".into(),
            "fetch".into(),
            "--no-recurse-submodules".into(),
            "origin".into(),
            "--".into(),
            requested_ref.into(),
        ],
    }
}

pub fn git_checkout_detached_spec(cache_path: &Path, commit: &str) -> GitCommandSpec {
    GitCommandSpec {
        program: "git".into(),
        args: vec![
            "-C".into(),
            cache_path.display().to_string(),
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "protocol.ext.allow=never".into(),
            "checkout".into(),
            "--detach".into(),
            commit.into(),
        ],
    }
}
