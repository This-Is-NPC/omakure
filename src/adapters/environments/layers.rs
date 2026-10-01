use super::values::{expand_env_value, should_mask_env_value, strip_quotes};
use super::{FsEnvironmentRepository, MASKED_ENV_VALUE, load_active_env_name};
use crate::error::{AppResult, EnvironmentError};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

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
/// lowercases keys for case-insensitive field lookups). Real environment variables
/// such as `PATH` and `VIRTUAL_ENV` are case-sensitive on Linux, so keys are
/// preserved verbatim here.
///
/// Line handling (comments, `export ` prefix, quote stripping, empty-value
/// skipping) mirrors [`parse_env_defaults`]. Values are returned **raw** —
/// `$VAR` / `${VAR}` expansion is deferred to [`merge_env_layers`], which
/// sources references from the parent shell plus prior user-provided layers,
/// per `docs/internal/env-injection-spec.md` §2. Reserved vars are injected later by
/// the executor and are not visible to this expansion step.
pub(super) fn parse_env_pairs_raw(contents: &str) -> Vec<(String, String)> {
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
pub(super) fn merge_env_layers(
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
