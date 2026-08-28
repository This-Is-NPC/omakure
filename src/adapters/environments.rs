use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{AppResult, EnvironmentError};
pub use crate::ports::{EnvFile, EnvironmentConfig};
use crate::ports::{EnvPreview, EnvironmentRepository};
use crate::util::{read_dir_or_empty, read_file_if_exists};

pub(crate) const MASKED_ENV_VALUE: &str = "****";
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct FsEnvironmentRepository {
    envs_dir: PathBuf,
}

impl FsEnvironmentRepository {
    pub fn new<P: Into<PathBuf>>(envs_dir: P) -> Self {
        Self {
            envs_dir: envs_dir.into(),
        }
    }

    fn read_env_defaults(&self, path: &Path) -> AppResult<HashMap<String, String>> {
        let contents = fs::read_to_string(path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?;
        Ok(parse_env_defaults(&contents))
    }

    pub(crate) fn env_path_for_name(&self, name: &str, must_exist: bool) -> AppResult<PathBuf> {
        validate_env_name(name)?;
        fs::create_dir_all(&self.envs_dir).map_err(|err| {
            EnvironmentError::WriteFailed(format!(
                "Failed to create environments dir {}: {}",
                self.envs_dir.display(),
                err
            ))
        })?;

        let path = self.envs_dir.join(format!("{name}.conf"));
        ensure_env_path_safe(&self.envs_dir, &path, must_exist)?;
        Ok(path)
    }
}

impl EnvironmentRepository for FsEnvironmentRepository {
    fn list_env_files(&self) -> AppResult<Vec<EnvFile>> {
        let mut entries = Vec::new();
        let dir = read_dir_or_empty(&self.envs_dir).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environments dir {}: {}",
                self.envs_dir.display(),
                err
            ))
        })?;

        for entry in dir {
            let path = entry.path();
            if path
                .symlink_metadata()
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };
            if name == "active" {
                continue;
            }
            entries.push(EnvFile { name });
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn load_environment_config(&self) -> AppResult<EnvironmentConfig> {
        let active = load_active_env_name(&self.envs_dir)?;
        let defaults = if let Some(name) = &active {
            let path = self.envs_dir.join(name);
            if !path.is_file() {
                return Err(EnvironmentError::NotFound {
                    name: path.display().to_string(),
                }
                .into());
            }
            self.read_env_defaults(&path)?
        } else {
            HashMap::new()
        };

        Ok(EnvironmentConfig {
            envs_dir: self.envs_dir.clone(),
            active,
            defaults,
        })
    }

    fn set_active_env(&self, name: Option<&str>) -> AppResult<()> {
        fs::create_dir_all(&self.envs_dir).map_err(|err| {
            EnvironmentError::WriteFailed(format!(
                "Failed to create environments dir {}: {}",
                self.envs_dir.display(),
                err
            ))
        })?;
        let active_path = self.envs_dir.join("active");

        match name {
            Some(name) => {
                let candidate = self.envs_dir.join(name);
                if !candidate.is_file() {
                    return Err(EnvironmentError::NotFound {
                        name: candidate.display().to_string(),
                    }
                    .into());
                }
                write_active_atomic(&active_path, name)?;
            }
            None => {
                if active_path.exists() {
                    fs::remove_file(&active_path).map_err(|err| {
                        EnvironmentError::WriteFailed(format!(
                            "Failed to clear active environment {}: {}",
                            active_path.display(),
                            err
                        ))
                    })?;
                }
            }
        }

        Ok(())
    }

    fn load_env_preview(&self, path: &Path) -> AppResult<EnvPreview> {
        let contents = fs::read_to_string(path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?;
        Ok(parse_env_preview(&contents))
    }

    fn create_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()> {
        let path = self.env_path_for_name(name, false)?;
        if path.exists() {
            return Err(EnvironmentError::WriteFailed(format!(
                "Environment already exists: {}",
                path.display()
            ))
            .into());
        }
        write_env_params_atomic(&path, params)
    }

    fn load_env_preview_by_name(&self, name: &str) -> AppResult<EnvPreview> {
        let path = self.env_path_for_name(name, true)?;
        self.load_env_preview(&path)
    }

    fn replace_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()> {
        let path = self.env_path_for_name(name, true)?;
        write_env_params_atomic(&path, params)
    }

    fn set_env_param(&self, name: &str, key: &str, value: &str) -> AppResult<()> {
        validate_env_key(key)?;
        let path = self.env_path_for_name(name, true)?;
        let mut params = parse_env_pairs_raw(&fs::read_to_string(&path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?);
        match params.iter_mut().find(|(existing, _)| existing == key) {
            Some((_, existing_value)) => *existing_value = value.to_string(),
            None => params.push((key.to_string(), value.to_string())),
        }
        let refs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        write_env_params_atomic(&path, &refs)
    }

    fn remove_env_param(&self, name: &str, key: &str) -> AppResult<()> {
        validate_env_key(key)?;
        let path = self.env_path_for_name(name, true)?;
        let mut params = parse_env_pairs_raw(&fs::read_to_string(&path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?);
        params.retain(|(existing, _)| existing != key);
        let refs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        write_env_params_atomic(&path, &refs)
    }

    fn activate_env(&self, name: &str) -> AppResult<()> {
        let path = self.env_path_for_name(name, true)?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| EnvironmentError::UnsafePath {
                path: path.display().to_string(),
            })?;
        write_active_atomic(&self.envs_dir.join("active"), file_name)
    }

    fn deactivate_env(&self) -> AppResult<()> {
        self.set_active_env(None)
    }

    fn delete_env(&self, name: &str) -> AppResult<()> {
        let path = self.env_path_for_name(name, true)?;
        fs::remove_file(&path).map_err(|err| {
            EnvironmentError::WriteFailed(format!(
                "Failed to delete environment file {}: {}",
                path.display(),
                err
            ))
        })?;

        if load_active_env_name(&self.envs_dir)? == Some(format!("{name}.conf")) {
            self.set_active_env(None)?;
        }
        Ok(())
    }
}

fn load_active_env_name(envs_dir: &Path) -> AppResult<Option<String>> {
    let active_path = envs_dir.join("active");
    let contents = read_file_if_exists(&active_path)
        .map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read active environment {}: {}",
                active_path.display(),
                err
            ))
        })?
        .unwrap_or_default();

    if contents.is_empty() {
        return Ok(None);
    }

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        return Ok(Some(trimmed.to_string()));
    }

    Ok(None)
}

fn validate_env_name(name: &str) -> Result<(), EnvironmentError> {
    let invalid = name.is_empty()
        || name == "active"
        || name.starts_with('.')
        || name.ends_with(".conf")
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || Path::new(name).components().count() != 1;
    if invalid {
        return Err(EnvironmentError::InvalidName {
            name: name.to_string(),
        });
    }
    Ok(())
}

fn validate_env_key(key: &str) -> Result<(), EnvironmentError> {
    if is_valid_var_name(key) {
        Ok(())
    } else {
        Err(EnvironmentError::WriteFailed(format!(
            "Invalid environment variable name: {key}"
        )))
    }
}

fn ensure_env_path_safe(envs_dir: &Path, path: &Path, must_exist: bool) -> AppResult<()> {
    let envs = envs_dir.canonicalize().map_err(|err| {
        EnvironmentError::ReadFailed(format!(
            "Failed to resolve environments dir {}: {}",
            envs_dir.display(),
            err
        ))
    })?;

    if must_exist && !path.is_file() {
        return Err(EnvironmentError::NotFound {
            name: path.display().to_string(),
        }
        .into());
    }

    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(EnvironmentError::UnsafePath {
                path: path.display().to_string(),
            }
            .into());
        }
    }

    let parent = path
        .parent()
        .unwrap_or(envs_dir)
        .canonicalize()
        .map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to resolve environment parent {}: {}",
                path.display(),
                err
            ))
        })?;
    if parent != envs {
        return Err(EnvironmentError::UnsafePath {
            path: path.display().to_string(),
        }
        .into());
    }

    Ok(())
}

fn write_env_params_atomic(path: &Path, params: &[(&str, &str)]) -> AppResult<()> {
    let mut contents = String::new();
    for (key, value) in params {
        validate_env_key(key)?;
        if value.contains('\n') || value.contains('\r') {
            return Err(EnvironmentError::WriteFailed(format!(
                "Environment value for {key} must be single-line"
            ))
            .into());
        }
        contents.push_str(key);
        contents.push('=');
        contents.push_str(value);
        contents.push('\n');
    }
    write_file_atomic(path, contents.as_bytes())
}

fn write_active_atomic(path: &Path, name: &str) -> AppResult<()> {
    write_file_atomic(path, format!("{name}\n").as_bytes())
}

fn write_file_atomic(path: &Path, contents: &[u8]) -> AppResult<()> {
    let parent = path.parent().ok_or_else(|| EnvironmentError::UnsafePath {
        path: path.display().to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to create environments dir {}: {}",
            parent.display(),
            err
        ))
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("env");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed),
        nonce
    ));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&tmp).map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to write temporary environment file {}: {}",
            tmp.display(),
            err
        ))
    })?;
    file.write_all(contents).map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to write temporary environment file {}: {}",
            tmp.display(),
            err
        ))
    })?;
    file.sync_all().map_err(|err| {
        EnvironmentError::WriteFailed(format!(
            "Failed to sync temporary environment file {}: {}",
            tmp.display(),
            err
        ))
    })?;
    drop(file);

    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        EnvironmentError::WriteFailed(format!(
            "Failed to replace environment file {}: {}",
            path.display(),
            err
        ))
    })?;
    Ok(())
}

pub(crate) fn parse_env_preview(contents: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();

    for line in contents.lines() {
        let mut trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix("export ") {
            trimmed = stripped.trim();
        }

        let mut parts = trimmed.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let raw_value = parts.next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let mut value = strip_quotes(raw_value).trim().to_string();
        if should_mask_env_value(key, &value) {
            value = MASKED_ENV_VALUE.to_string();
        }
        entries.push((key.to_string(), value));
    }

    entries
}

pub(crate) fn parse_env_defaults(contents: &str) -> HashMap<String, String> {
    let mut defaults = HashMap::new();

    for line in contents.lines() {
        let mut trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix("export ") {
            trimmed = stripped.trim();
        }

        let mut parts = trimmed.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let raw_value = parts.next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let value = strip_quotes(raw_value).trim();
        if value.is_empty() {
            continue;
        }
        defaults.insert(key.to_ascii_lowercase(), value.to_string());
    }

    defaults
}

/// Parse env-file `contents` into ordered, **case-preserving**, *unexpanded*
/// key/value pairs.
///
/// This is a deliberately separate path from [`parse_env_defaults`] (which
/// lowercases keys for legacy schema-field prefill). Real environment variables
/// such as `PATH` and `VIRTUAL_ENV` are case-sensitive on Linux, so keys are
/// preserved verbatim here.
///
/// Line handling (comments, `export ` prefix, quote stripping, empty-value
/// skipping) mirrors [`parse_env_defaults`]. Values are returned **raw** —
/// `$VAR` / `${VAR}` expansion is deferred to [`merge_env_layers`], which
/// sources references from the parent shell plus prior user-provided layers,
/// per `docs/internal/env-injection-spec.md` §2. Reserved vars are injected later by
/// the executor and are not visible to this expansion step.
fn parse_env_pairs_raw(contents: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();

    for line in contents.lines() {
        let mut trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix("export ") {
            trimmed = stripped.trim();
        }

        let mut parts = trimmed.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let raw_value = parts.next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let value = strip_quotes(raw_value).trim();
        if value.is_empty() {
            continue;
        }
        // Preserve original key case verbatim.
        pairs.push((key.to_string(), value.to_string()));
    }

    pairs
}

/// The parent shell environment, used **only** as a lower-precedence
/// expansion source (`docs/internal/env-injection-spec.md` §1 layer 1). It is never
/// emitted as an injected pair — see [`merge_env_layers`].
fn parent_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// Expand and merge raw env `layers` (ordered lowest → highest precedence) on
/// top of `base`, applying the single-pass `$VAR` / `${VAR}` grammar
/// (`docs/internal/env-injection-spec.md` §2).
///
/// `base` carries lower-precedence env used **only** as an expansion source
/// (the parent shell env; see [`parent_env`]). Each value is expanded against
/// the accumulator *before* its key is written back, so a self-referencing
/// value like `PATH=/x/bin:$PATH` prepends to the inherited PATH instead of
/// referencing the file's own raw value (which would double the prefix and
/// leave a literal `$PATH`). A later layer's value therefore also sees the
/// already-expanded value from an earlier layer.
///
/// The returned vec contains **only** keys drawn from `layers` (never `base`),
/// in first-seen order; a later layer overrides an earlier key **in place**.
/// This keeps the parent shell env out of `extra_env` — the child inherits it
/// automatically — while still using it to resolve references.
fn merge_env_layers(
    base: &HashMap<String, String>,
    layers: &[&[(String, String)]],
) -> Vec<(String, String)> {
    let mut env = base.clone();
    let mut out: Vec<(String, String)> = Vec::new();

    for layer in layers {
        for (key, raw) in *layer {
            let expanded = expand_env_value(raw, &env);
            env.insert(key.clone(), expanded.clone());
            match out.iter_mut().find(|(k, _)| k == key) {
                Some(existing) => existing.1 = expanded,
                None => out.push((key.clone(), expanded)),
            }
        }
    }

    out
}

/// Read the managed active env into raw, unexpanded pairs (best-effort: an
/// absent `active` pointer or unreadable target yields an empty vec). Shared
/// by [`resolve_active_env`] and [`resolve_run_env`] so the active-read path
/// has a single implementation.
fn active_env_raw(envs_dir: &Path) -> Vec<(String, String)> {
    let Ok(Some(name)) = load_active_env_name(envs_dir) else {
        return Vec::new();
    };
    let logical = name.strip_suffix(".conf").unwrap_or(&name);
    let repo = FsEnvironmentRepository::new(envs_dir.to_path_buf());
    let Ok(path) = repo.env_path_for_name(logical, true) else {
        return Vec::new();
    };
    match fs::read_to_string(path) {
        Ok(contents) => parse_env_pairs_raw(&contents),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn read_managed_env_defaults(
    envs_dir: &Path,
    name: &str,
) -> AppResult<HashMap<String, String>> {
    let repo = FsEnvironmentRepository::new(envs_dir.to_path_buf());
    let path = repo.env_path_for_name(name, true)?;
    repo.read_env_defaults(&path)
}

/// Resolve the active managed environment into ordered, case-preserving
/// `KEY=value` pairs for injection as `extra_env` into a spawned script
/// process.
///
/// This is the single composition root for env injection: all three run
/// call sites (CLI `omakure run` and the queue worker) call this function to
/// build their `extra_env`, so there is one merge
/// implementation, not three.
///
/// It implements **layer 2** of the env-injection precedence table
/// (`docs/internal/env-injection-spec.md` §1): the managed active env selected by
/// `.omakure/envs/active`, read from `.omakure/envs/<name>.conf` and parsed
/// case-sensitively via [`parse_env_pairs_raw`], then expanded and merged by
/// [`merge_env_layers`] on top of the parent shell env (layer 1). The
/// remaining layers are handled by the run path so later layers always win per
/// key:
///
/// - **Layer 1** (parent shell env) is inherited by the child automatically
///   and is overridden by any key returned here. It is **also** the base
///   expansion source, so a value like `PATH=/x/bin:$PATH` prepends to the
///   inherited PATH (and does not leak parent keys into the returned pairs).
/// - **Layer 3** (CLI `--env-file`) is composed by [`resolve_run_env`], which
///   re-reads the active env and folds the env-file layer on top in one merge.
/// - **Layer 4** (`OMAKURE_RUN_ID` / `OMAKURE_SCRIPTS_DIR`) is pushed onto
///   `extra_env` *after* these pairs in
///   [`crate::run_executor::execute_with_heartbeat`], and is therefore
///   **non-overridable**: a user key of the same name from this env file
///   cannot clobber the reserved value.
///
/// Behavior change (was: prefill-only): prior to this, `.omakure/envs/*.conf`
/// only provided legacy schema-field defaults and never reached the spawned
/// process. Those files now inject into the child's `os.environ`. There is
/// no CHANGELOG file in this repo, so this doc-comment records the change.
///
/// Injection is best-effort: an absent `active` pointer or an unreadable env
/// file yields an empty vec rather than failing the run. Per spec §3 the
/// returned pairs reach only the spawned process env (`cmd.env`); they are
/// never persisted to `runs.sqlite`, logs, or the trace.
pub(crate) fn resolve_active_env(envs_dir: &Path) -> Vec<(String, String)> {
    merge_env_layers(&parent_env(), &[&active_env_raw(envs_dir)])
}

/// Resolve the full per-run `extra_env` for a `omakure run` invocation:
/// layer 2 (managed active env) with an optional layer 3 (CLI `--env-file`)
/// folded **on top** (`docs/internal/env-injection-spec.md` §1).
///
/// This is the single composition root for the layer-2 + layer-3 merge so the
/// precedence logic lives in exactly one place, not inline at the call site.
/// Per key (compared case-sensitively) the `--env-file` value **overrides**
/// the active-env value; a key present in only one source is kept. The
/// reserved layer-4 vars (`OMAKURE_RUN_ID`, `OMAKURE_SCRIPTS_DIR`) are pushed
/// *after* this vec in [`crate::run_executor::execute_with_heartbeat`] and so
/// remain non-overridable by either layer here.
///
/// Unlike [`resolve_active_env`] (best-effort — an absent active env yields an
/// empty vec), an `env_file` path the caller **explicitly** passed that cannot
/// be read is a hard error: silently ignoring a user-supplied path would hide
/// typos and stale references.
pub(crate) fn resolve_run_env(
    envs_dir: &Path,
    env_file: Option<&Path>,
) -> Result<Vec<(String, String)>, EnvironmentError> {
    let active = active_env_raw(envs_dir);
    let env_file_pairs = match env_file {
        Some(path) => {
            let contents = fs::read_to_string(path).map_err(|err| {
                EnvironmentError::ReadFailed(format!(
                    "Failed to read --env-file {}: {}",
                    path.display(),
                    err
                ))
            })?;
            parse_env_pairs_raw(&contents)
        }
        None => Vec::new(),
    };
    // Expand both layers against one growing map seeded with the parent env so
    // `$VAR` (incl. self-references) resolves against the merged env, and the
    // env-file layer sees the active layer's already-expanded values.
    Ok(merge_env_layers(&parent_env(), &[&active, &env_file_pairs]))
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Valid variable name: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if is_name_start(first) => chars.all(is_name_char),
        _ => false,
    }
}

/// Single-pass, non-recursive `$VAR` / `${VAR}` expansion per the
/// env-injection grammar (`docs/internal/env-injection-spec.md` section 2).
///
/// - The input is scanned left-to-right exactly once; substituted output is
///   never re-scanned (no recursion).
/// - `$VAR` bare form: the name is the longest run of `[A-Za-z0-9_]` after a
///   name-start (`[A-Za-z_]`).
/// - `${VAR}` braced form: the body between `{` and the next `}`. A body that
///   is not a valid name resolves as undefined. An unterminated `${...` is
///   emitted literally.
/// - Undefined references expand to the empty string.
/// - The only escape is `\$` -> literal `$`; any other `\` is literal.
/// - No command substitution: `$(...)` and backticks are emitted literally.
fn expand_env_value(input: &str, vars: &HashMap<String, String>) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if c == '\\' {
            // `\$` is the only escape unit; anything else is a literal `\`.
            if i + 1 < chars.len() && chars[i + 1] == '$' {
                out.push('$');
                i += 2;
            } else {
                out.push('\\');
                i += 1;
            }
            continue;
        }

        if c == '$' {
            // Braced form `${...}`.
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                if let Some(close) = (i + 2..chars.len()).find(|&j| chars[j] == '}') {
                    let name: String = chars[i + 2..close].iter().collect();
                    if is_valid_var_name(&name) {
                        out.push_str(vars.get(&name).map(String::as_str).unwrap_or(""));
                    }
                    // Invalid name -> undefined -> empty string (push nothing).
                    i = close + 1;
                } else {
                    // Unterminated `${...` -> literal passthrough to end.
                    out.extend(chars[i..].iter());
                    break;
                }
                continue;
            }

            // Bare form `$VAR`.
            if i + 1 < chars.len() && is_name_start(chars[i + 1]) {
                let mut j = i + 1;
                while j < chars.len() && is_name_char(chars[j]) {
                    j += 1;
                }
                let name: String = chars[i + 1..j].iter().collect();
                out.push_str(vars.get(&name).map(String::as_str).unwrap_or(""));
                i = j;
                continue;
            }

            // `$` not followed by a name-start or `{` -> literal `$`.
            out.push('$');
            i += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }

    out
}

fn strip_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0] as char;
        let last = trimmed.as_bytes()[trimmed.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

pub(crate) fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let tokens = [
        "password",
        "passwd",
        "pwd",
        "secret",
        "token",
        "key",
        "api",
        "private",
        "cred",
        "passphrase",
        "auth",
        "bearer",
    ];
    tokens.iter().any(|token| lower.contains(token))
}

pub(crate) fn value_contains_credentials(value: &str) -> bool {
    let Some(scheme_end) = value.find("://") else {
        return false;
    };
    if scheme_end == 0 {
        return false;
    }
    let authority = &value[scheme_end + 3..];
    let authority_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
    let authority = &authority[..authority_end];
    let Some(at) = authority.rfind('@') else {
        return false;
    };
    let userinfo = &authority[..at];
    let Some(colon) = userinfo.find(':') else {
        return false;
    };
    colon > 0 && colon + 1 < userinfo.len()
}

/// Decide whether a managed-env value should be masked (`****`) on read paths
/// (`env show`, `GET /v1/envs/:name`).
///
/// This is a best-effort **denylist heuristic** — it masks values whose key
/// looks sensitive ([`is_sensitive_key`]) or whose value embeds URL
/// credentials ([`value_contains_credentials`]). It CANNOT catch a real secret
/// stored under an innocuous key with an opaque value (e.g.
/// `DEPLOY_HOOK=T00xxxxSECRET`); such a value is returned in cleartext.
///
/// Managed envs are config, not a secret store: operators must put real
/// secrets behind `secret://` refs (env/file providers), which the redaction
/// pipeline covers end-to-end at rest and in output, rather than relying on
/// this display mask.
pub(crate) fn should_mask_env_value(key: &str, value: &str) -> bool {
    !value.is_empty() && (is_sensitive_key(key) || value_contains_credentials(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use std::fs;
    use tempfile::TempDir;

    // --- Pure function tests ---

    #[rstest]
    #[case::password("DB_PASSWORD", true)]
    #[case::secret("SECRET_KEY", true)]
    #[case::token("AUTH_TOKEN", true)]
    #[case::api_key("API_KEY", true)]
    #[case::private("PRIVATE_KEY", true)]
    #[case::cred("CREDENTIALS", true)]
    #[case::key("SSH_KEY", true)]
    #[case::passwd("MYSQL_PASSWD", true)]
    #[case::pwd("MYSQL_PWD", true)]
    #[case::passphrase("SSH_PASSPHRASE", true)]
    #[case::basic_auth("BASIC_AUTH", true)]
    #[case::authorization("AUTHORIZATION", true)]
    #[case::bearer("BEARER_HEADER", true)]
    #[case::url_not_sensitive("DATABASE_URL", false)]
    #[case::name_not_sensitive("APP_NAME", false)]
    #[case::port_not_sensitive("PORT", false)]
    #[case::debug_not_sensitive("DEBUG", false)]
    fn test_is_sensitive_key(#[case] key: &str, #[case] expected: bool) {
        assert_eq!(is_sensitive_key(key), expected);
    }

    #[rstest]
    #[case::simple_pair("KEY=value", vec![("key", "value")])]
    #[case::export_prefix("export KEY=value", vec![("key", "value")])]
    #[case::double_quotes("KEY=\"quoted value\"", vec![("key", "quoted value")])]
    #[case::single_quotes("KEY='single'", vec![("key", "single")])]
    #[case::comment_skipped("# comment", vec![])]
    #[case::semicolon_comment_skipped("; comment", vec![])]
    #[case::empty_line_skipped("", vec![])]
    #[case::empty_value_skipped("KEY=", vec![])]
    #[case::whitespace_trimmed("  KEY = value  ", vec![("key", "value")])]
    fn test_parse_env_defaults(#[case] input: &str, #[case] expected: Vec<(&str, &str)>) {
        let result = parse_env_defaults(input);
        let expected_map: HashMap<String, String> = expected
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(result, expected_map);
    }

    // CHARACTERIZATION: pins the CURRENT behavior of `parse_env_defaults`
    // (TUI schema-field prefill). Keys are LOWERCASED, quotes/`export ` are
    // stripped, and empty values are skipped. This guards the prefill path
    // against silent regression when the case-preserving parser is added.
    #[test]
    fn test_parse_env_defaults_characterization_lowercases_and_strips() {
        let input = concat!(
            "PATH=/usr/bin\n",
            "export VIRTUAL_ENV=\"/opt/venv\"\n",
            "Mixed_Case='value'\n",
            "# comment\n",
            "; also comment\n",
            "EMPTY=\n",
            "  SPACED  =  spaced value  \n",
        );
        let result = parse_env_defaults(input);

        // Keys are lowercased verbatim (the behavior injection must NOT use).
        assert_eq!(result.get("path").map(String::as_str), Some("/usr/bin"));
        assert_eq!(
            result.get("virtual_env").map(String::as_str),
            Some("/opt/venv")
        );
        assert_eq!(result.get("mixed_case").map(String::as_str), Some("value"));
        assert_eq!(
            result.get("spaced").map(String::as_str),
            Some("spaced value")
        );
        // Original-case keys are absent (proves lowercasing).
        assert!(!result.contains_key("PATH"));
        assert!(!result.contains_key("VIRTUAL_ENV"));
        // Comments and empty values are dropped.
        assert!(!result.contains_key("empty"));
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_parse_env_defaults_multiline() {
        let input = "HOST=localhost\nPORT=8080\n# comment\nDEBUG=true";
        let result = parse_env_defaults(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result.get("host").unwrap(), "localhost");
        assert_eq!(result.get("port").unwrap(), "8080");
        assert_eq!(result.get("debug").unwrap(), "true");
    }

    #[test]
    #[cfg(unix)]
    fn write_file_atomic_does_not_follow_predictable_temp_symlink() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        let target = envs.join("prod.conf");
        let outside = tmp.path().join("outside.conf");
        fs::write(&outside, "outside=original\n").unwrap();
        let old_predictable_tmp = envs.join(format!(".prod.conf.{}.tmp", std::process::id()));
        symlink(&outside, &old_predictable_tmp).unwrap();

        write_file_atomic(&target, b"TOKEN=secret\n").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "TOKEN=secret\n");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside=original\n");
        assert!(old_predictable_tmp
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[rstest]
    #[case::simple_pair("HOST=localhost", vec![("HOST", "localhost")])]
    #[case::sensitive_masked("DB_PASSWORD=secret123", vec![("DB_PASSWORD", "****")])]
    #[case::api_key_masked("API_KEY=abc", vec![("API_KEY", "****")])]
    #[case::credential_url_masked("DATABASE_URL=postgres://user:pass@localhost/db", vec![("DATABASE_URL", "****")])]
    #[case::comment_skipped("# comment\nNAME=test", vec![("NAME", "test")])]
    fn test_parse_env_preview(#[case] input: &str, #[case] expected: Vec<(&str, &str)>) {
        let result = parse_env_preview(input);
        let expected_vec: Vec<(String, String)> = expected
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(result, expected_vec);
    }

    // --- Case-preserving injectable parser + var expansion ---

    #[test]
    fn test_parse_env_pairs_raw_preserves_key_case() {
        let input = "PATH=/usr/bin\nVIRTUAL_ENV=/opt/venv\nMixed_Case=v";
        let result = parse_env_pairs_raw(input);
        assert_eq!(
            result,
            vec![
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("VIRTUAL_ENV".to_string(), "/opt/venv".to_string()),
                ("Mixed_Case".to_string(), "v".to_string()),
            ]
        );
    }

    #[test]
    fn test_parse_env_pairs_raw_line_handling_matches_defaults() {
        // export prefix, quotes, comments, empty-value skipping — but ordered
        // and case-preserved.
        let input = concat!(
            "export FOO=\"bar\"\n",
            "# comment\n",
            "; comment\n",
            "EMPTY=\n",
            "  SP  =  spaced value  \n",
            "SINGLE='q'\n",
        );
        let result = parse_env_pairs_raw(input);
        assert_eq!(
            result,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("SP".to_string(), "spaced value".to_string()),
                ("SINGLE".to_string(), "q".to_string()),
            ]
        );
    }

    #[test]
    fn test_merge_env_layers_expands_bare_and_braced_within_layer() {
        // BASE defined first; later values reference it in both forms.
        let input = concat!("BASE=/opt\n", "BARE=$BASE/bin\n", "BRACED=${BASE}/lib\n",);
        let result = merge_env_layers(&HashMap::new(), &[&parse_env_pairs_raw(input)]);
        assert_eq!(
            result,
            vec![
                ("BASE".to_string(), "/opt".to_string()),
                ("BARE".to_string(), "/opt/bin".to_string()),
                ("BRACED".to_string(), "/opt/lib".to_string()),
            ]
        );
    }

    // --- merge_env_layers: expansion sources the merged env incl. parent ---
    // (regression coverage for task 1758)

    #[test]
    fn test_merge_env_layers_self_reference_prepends_to_base_path() {
        // `PATH=/x/bin:$PATH` must prepend to the base (parent) PATH, not
        // self-reference the file's own raw value. No doubled prefix, no
        // literal `$PATH` residue, system PATH preserved.
        let base = vars(&[("PATH", "/usr/bin:/bin")]);
        let layer = vec![("PATH".to_string(), "/x/bin:$PATH".to_string())];
        assert_eq!(
            merge_env_layers(&base, &[&layer]),
            vec![("PATH".to_string(), "/x/bin:/usr/bin:/bin".to_string())]
        );
    }

    #[test]
    fn test_merge_env_layers_returns_only_layer_keys_not_base() {
        // The base (parent shell env) is an expansion SOURCE only; it must
        // never leak into the emitted pairs.
        let base = vars(&[("PATH", "/usr/bin"), ("SECRET_TOKEN", "shh")]);
        let layer = vec![("MY_VAR".to_string(), "hello".to_string())];
        assert_eq!(
            merge_env_layers(&base, &[&layer]),
            vec![("MY_VAR".to_string(), "hello".to_string())]
        );
    }

    #[test]
    fn test_merge_env_layers_undefined_in_file_and_base_is_empty() {
        // A var absent from both the file layers AND the base expands empty.
        let base = vars(&[("PATH", "/usr/bin")]);
        let layer = vec![("X".to_string(), "a${MISSING}b".to_string())];
        assert_eq!(
            merge_env_layers(&base, &[&layer]),
            vec![("X".to_string(), "ab".to_string())]
        );
    }

    #[test]
    fn test_merge_env_layers_later_layer_expands_against_earlier() {
        // The env-file layer (higher precedence) sees the active layer's
        // already-expanded value and overrides the key in place.
        let base = vars(&[("PATH", "/sys")]);
        let active = vec![("PATH".to_string(), "/active:$PATH".to_string())];
        let file = vec![("PATH".to_string(), "/file:$PATH".to_string())];
        assert_eq!(
            merge_env_layers(&base, &[&active, &file]),
            vec![("PATH".to_string(), "/file:/active:/sys".to_string())]
        );
    }

    #[test]
    fn test_merge_env_layers_file_key_overrides_base_no_ref() {
        // A file key with no `$` reference simply overrides the base value and
        // is emitted verbatim (base value is not carried through).
        let base = vars(&[("HOST", "parent")]);
        let layer = vec![("HOST".to_string(), "fromfile".to_string())];
        assert_eq!(
            merge_env_layers(&base, &[&layer]),
            vec![("HOST".to_string(), "fromfile".to_string())]
        );
    }

    // --- resolve_active_env (layer 2 injector, spec section 1) ---

    #[test]
    fn test_resolve_active_env_none_when_no_active_pointer() {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        fs::write(envs.join("dev.conf"), "HOST=localhost").unwrap();
        // No `active` pointer => nothing to inject.
        assert!(resolve_active_env(&envs).is_empty());
    }

    #[test]
    fn test_resolve_active_env_reads_active_conf_case_preserving() {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        fs::write(envs.join("dev.conf"), "PATH=/usr/bin\nMY_VAR=hello").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        // Keys are preserved verbatim (unlike the lowercasing prefill path)
        // and order is stable.
        assert_eq!(
            resolve_active_env(&envs),
            vec![
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("MY_VAR".to_string(), "hello".to_string()),
            ]
        );
    }

    #[test]
    fn test_resolve_active_env_missing_conf_is_best_effort_empty() {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        fs::write(envs.join("active"), "ghost.conf\n").unwrap();
        // Unreadable/missing target must not fail the run — resolves empty.
        assert!(resolve_active_env(&envs).is_empty());
    }

    // --- resolve_run_env (layers 2 + 3 composition root, spec section 1) ---

    #[test]
    fn test_resolve_run_env_no_env_file_equals_active_env() {
        // With no --env-file, resolve_run_env is exactly the active env.
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        fs::write(envs.join("dev.conf"), "HOST=localhost\nPORT=8080").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        assert_eq!(
            resolve_run_env(&envs, None).unwrap(),
            resolve_active_env(&envs)
        );
    }

    #[test]
    fn test_resolve_run_env_env_file_overrides_active_env() {
        // Layer 3 (--env-file) wins over layer 2 (active env) for the same
        // case-sensitive key; a key only in the env-file is appended; a key
        // only in the active env is preserved.
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        fs::write(envs.join("dev.conf"), "HOST=active\nONLY_ACTIVE=keep").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let env_file = tmp.path().join("run.env");
        fs::write(&env_file, "HOST=fromfile\nONLY_FILE=added").unwrap();

        let merged = resolve_run_env(&envs, Some(&env_file)).unwrap();
        // HOST overridden in place; active-only preserved; file-only appended.
        assert_eq!(
            merged,
            vec![
                ("HOST".to_string(), "fromfile".to_string()),
                ("ONLY_ACTIVE".to_string(), "keep".to_string()),
                ("ONLY_FILE".to_string(), "added".to_string()),
            ]
        );
    }

    #[test]
    fn test_resolve_run_env_env_file_only_no_active() {
        // No active env: the env-file pairs are the whole result.
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();

        let env_file = tmp.path().join("run.env");
        fs::write(&env_file, "TOKEN=abc").unwrap();

        assert_eq!(
            resolve_run_env(&envs, Some(&env_file)).unwrap(),
            vec![("TOKEN".to_string(), "abc".to_string())]
        );
    }

    #[test]
    fn test_resolve_run_env_missing_env_file_is_error() {
        // An explicit --env-file path the user passed that does not exist
        // must be a hard error, not a silent skip.
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();

        let ghost = tmp.path().join("does-not-exist.env");
        let err = resolve_run_env(&envs, Some(&ghost)).unwrap_err();
        assert!(
            err.to_string().contains("does-not-exist.env"),
            "error should name the offending path, got: {}",
            err
        );
    }

    // --- expand_env_value grammar (spec section 2.6 worked examples) ---

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[rstest]
    #[case::bare("$FOO", "bar")]
    #[case::braced("${FOO}", "bar")]
    #[case::bare_suffix("$FOO/baz", "bar/baz")]
    #[case::braced_suffix("${FOO}baz", "barbaz")]
    #[case::undefined_bare("a$BAZ", "a")]
    #[case::undefined_braced("x${BAZ}y", "xy")]
    #[case::escaped_dollar("\\$FOO", "$FOO")]
    #[case::literal_backslash_then_escaped_dollar("\\\\$FOO", "\\$FOO")]
    #[case::command_sub_literal("$(echo hi)", "$(echo hi)")]
    #[case::backtick_literal("`date`", "`date`")]
    #[case::rich_form_empty("${FOO:-x}", "")]
    #[case::digit_literal("$1abc", "$1abc")]
    #[case::unterminated_brace("${FOO", "${FOO")]
    #[case::bare_no_name("$ ", "$ ")]
    #[case::backslash_literal("a\\b", "a\\b")]
    fn test_expand_env_value_grammar(#[case] input: &str, #[case] expected: &str) {
        let env = vars(&[("FOO", "bar"), ("OMAKURE_RUN_ID", "r-1")]);
        assert_eq!(expand_env_value(input, &env), expected);
    }

    #[test]
    fn test_expand_env_value_no_recursion() {
        // FOO expands to a literal that itself looks like a reference; the
        // output must NOT be re-scanned.
        let env = vars(&[("FOO", "$BAR"), ("BAR", "deep")]);
        assert_eq!(expand_env_value("$FOO", &env), "$BAR");
    }

    #[test]
    fn test_merge_env_layers_command_substitution_not_executed() {
        let input = "CMD=$(rm -rf /)\nTICK=`date`";
        let result = merge_env_layers(&HashMap::new(), &[&parse_env_pairs_raw(input)]);
        assert_eq!(
            result,
            vec![
                ("CMD".to_string(), "$(rm -rf /)".to_string()),
                ("TICK".to_string(), "`date`".to_string()),
            ]
        );
    }

    // --- Filesystem-based tests ---

    #[fixture]
    fn envs_dir() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();

        fs::write(envs.join("dev.conf"), "HOST=localhost\nPORT=3000").unwrap();
        fs::write(
            envs.join("prod.conf"),
            "HOST=prod.example.com\nAPI_KEY=secret",
        )
        .unwrap();

        (tmp, envs)
    }

    #[rstest]
    fn test_list_env_files(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);
        let files = repo.list_env_files().unwrap();

        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["dev.conf", "prod.conf"]);
    }

    #[rstest]
    fn test_list_env_files_skips_active(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        let files = repo.list_env_files().unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert!(!names.contains(&"active"));
    }

    #[rstest]
    fn test_load_environment_config_no_active(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);
        let config = repo.load_environment_config().unwrap();

        assert!(config.active.is_none());
        assert!(config.defaults.is_empty());
    }

    #[rstest]
    fn test_load_environment_config_with_active(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        let config = repo.load_environment_config().unwrap();

        assert_eq!(config.active, Some("dev.conf".to_string()));
        assert_eq!(config.defaults.get("host").unwrap(), "localhost");
        assert_eq!(config.defaults.get("port").unwrap(), "3000");
    }

    #[test]
    #[cfg(unix)]
    fn resolve_active_env_ignores_symlink_target() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        let outside = tmp.path().join("outside.conf");
        fs::write(&outside, "TOKEN=outside_secret").unwrap();
        symlink(&outside, envs.join("prod.conf")).unwrap();
        fs::write(envs.join("active"), "prod.conf\n").unwrap();

        let resolved = resolve_active_env(&envs);

        assert!(resolved.is_empty());
    }

    #[rstest]
    fn test_set_active_env(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);

        repo.set_active_env(Some("dev.conf")).unwrap();
        let active = fs::read_to_string(envs.join("active")).unwrap();
        assert_eq!(active.trim(), "dev.conf");
    }

    #[rstest]
    fn test_set_active_env_clear(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        repo.set_active_env(None).unwrap();
        assert!(!envs.join("active").exists());
    }

    #[rstest]
    fn test_set_active_env_nonexistent(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);

        let result = repo.set_active_env(Some("nonexistent.conf"));
        assert!(result.is_err());
    }

    #[rstest]
    fn test_load_environment_config_active_points_to_missing_file(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "ghost.conf\n").unwrap();
        let repo = FsEnvironmentRepository::new(&envs);
        let err = repo.load_environment_config().unwrap_err();
        assert!(format!("{}", err).contains("Environment not found"));
    }

    #[rstest]
    fn test_load_active_env_name_skips_comments_only(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "# just a comment\n; also comment\n").unwrap();
        let repo = FsEnvironmentRepository::new(&envs);
        let config = repo.load_environment_config().unwrap();
        assert!(config.active.is_none());
    }

    #[test]
    fn test_parse_env_preview_strips_export_prefix() {
        let input = "export GREETING=hello";
        let preview = parse_env_preview(input);
        assert_eq!(preview, vec![("GREETING".to_string(), "hello".to_string())]);
    }

    #[test]
    fn test_parse_env_defaults_strips_export_prefix() {
        let input = "export FOO=bar";
        let parsed = parse_env_defaults(input);
        assert_eq!(parsed.get("foo").map(String::as_str), Some("bar"));
    }

    #[rstest]
    fn test_load_env_preview(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);
        let preview = repo.load_env_preview(&envs.join("prod.conf")).unwrap();

        assert_eq!(
            preview[0],
            ("HOST".to_string(), "prod.example.com".to_string())
        );
        assert_eq!(preview[1], ("API_KEY".to_string(), "****".to_string()));
    }

    #[rstest]
    fn test_create_show_replace_set_remove_and_delete_env_by_logical_name(
        envs_dir: (TempDir, PathBuf),
    ) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);

        repo.create_env("qa", &[("HOST", "qa.example.com"), ("API_KEY", "secret")])
            .unwrap();
        assert!(envs.join("qa.conf").is_file());

        assert_eq!(
            repo.load_env_preview_by_name("qa").unwrap(),
            vec![
                ("HOST".to_string(), "qa.example.com".to_string()),
                ("API_KEY".to_string(), "****".to_string()),
            ]
        );

        repo.set_env_param("qa", "PORT", "443").unwrap();
        repo.set_env_param("qa", "HOST", "qa.internal").unwrap();
        assert_eq!(
            fs::read_to_string(envs.join("qa.conf")).unwrap(),
            "HOST=qa.internal\nAPI_KEY=secret\nPORT=443\n"
        );

        repo.remove_env_param("qa", "API_KEY").unwrap();
        assert_eq!(
            fs::read_to_string(envs.join("qa.conf")).unwrap(),
            "HOST=qa.internal\nPORT=443\n"
        );

        repo.replace_env("qa", &[("HOST", "replacement")]).unwrap();
        assert_eq!(
            fs::read_to_string(envs.join("qa.conf")).unwrap(),
            "HOST=replacement\n"
        );

        repo.delete_env("qa").unwrap();
        assert!(!envs.join("qa.conf").exists());
    }

    #[rstest]
    #[case::empty("")]
    #[case::suffix("prod.conf")]
    #[case::traversal("../prod")]
    #[case::slash("team/prod")]
    #[case::backslash("team\\prod")]
    #[case::leading_dot(".prod")]
    #[case::reserved_active("active")]
    fn test_env_management_rejects_unsafe_logical_names(
        envs_dir: (TempDir, PathBuf),
        #[case] name: &str,
    ) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);

        let err = repo.create_env(name, &[("HOST", "example")]).unwrap_err();
        assert!(
            err.to_string().contains("Invalid environment name"),
            "unexpected error for {name:?}: {err}"
        );
    }

    #[rstest]
    fn test_env_management_rejects_symlink_escape(envs_dir: (TempDir, PathBuf)) {
        let (tmp, envs) = envs_dir;
        let outside = tmp.path().join("outside.conf");
        fs::write(&outside, "HOST=outside\n").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, envs.join("escape.conf")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, envs.join("escape.conf")).unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        let err = repo.load_env_preview_by_name("escape").unwrap_err();
        assert!(
            err.to_string().contains("Unsafe environment path"),
            "unexpected error: {err}"
        );
    }

    #[rstest]
    fn test_env_management_activate_deactivate_uses_logical_name(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);

        repo.activate_env("prod").unwrap();
        assert_eq!(
            fs::read_to_string(envs.join("active")).unwrap(),
            "prod.conf\n"
        );

        repo.deactivate_env().unwrap();
        assert!(!envs.join("active").exists());
    }

    #[rstest]
    #[case::postgres_password("postgres://user:pass@localhost/db", true)]
    #[case::https_basic_auth("https://user:pass@example.com/path", true)]
    #[case::no_password("postgres://user@localhost/db", false)]
    #[case::no_user("postgres://localhost/db", false)]
    #[case::not_url("user:pass@localhost", false)]
    fn test_value_contains_credentials(#[case] value: &str, #[case] expected: bool) {
        assert_eq!(value_contains_credentials(value), expected);
    }
}
