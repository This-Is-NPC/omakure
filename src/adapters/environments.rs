use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppResult, EnvironmentError};
pub use crate::ports::{EnvFile, EnvironmentConfig};
use crate::ports::{EnvPreview, EnvironmentRepository};
use crate::util::{read_dir_or_empty, read_file_if_exists};

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
            session_conf_path: None,
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
                fs::write(&active_path, format!("{}\n", name)).map_err(|err| {
                    EnvironmentError::WriteFailed(format!(
                        "Failed to write active environment {}: {}",
                        active_path.display(),
                        err
                    ))
                })?;
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
        if is_sensitive_key(key) && !value.is_empty() {
            value = "***".to_string();
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
/// lowercases keys for TUI schema-field prefill). Real environment variables
/// such as `PATH` and `VIRTUAL_ENV` are case-sensitive on Linux, so keys are
/// preserved verbatim here.
///
/// Line handling (comments, `export ` prefix, quote stripping, empty-value
/// skipping) mirrors [`parse_env_defaults`]. Values are returned **raw** —
/// `$VAR` / `${VAR}` expansion is deferred to [`merge_env_layers`], which
/// sources references from the fully merged env (parent shell + all layers),
/// per `.docs/env-injection-spec.md` §2.
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
/// expansion source (`.docs/env-injection-spec.md` §1 layer 1). It is never
/// emitted as an injected pair — see [`merge_env_layers`].
fn parent_env() -> HashMap<String, String> {
    std::env::vars().collect()
}

/// Expand and merge raw env `layers` (ordered lowest → highest precedence) on
/// top of `base`, applying the single-pass `$VAR` / `${VAR}` grammar
/// (`.docs/env-injection-spec.md` §2).
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
    match fs::read_to_string(envs_dir.join(&name)) {
        Ok(contents) => parse_env_pairs_raw(&contents),
        Err(_) => Vec::new(),
    }
}

/// Resolve the active managed environment into ordered, case-preserving
/// `KEY=value` pairs for injection as `extra_env` into a spawned script
/// process.
///
/// This is the single composition root for env injection: all three run
/// call sites (CLI `omakure run`, the queue worker, and the TUI inline run)
/// call this function to build their `extra_env`, so there is one merge
/// implementation, not three.
///
/// It implements **layer 2** of the env-injection precedence table
/// (`.docs/env-injection-spec.md` §1): the managed active env selected by
/// `.omaken/envs/active`, read from `.omaken/envs/<name>.conf` and parsed
/// case-sensitively via [`parse_env_pairs_raw`], then expanded and merged by
/// [`merge_env_layers`] on top of the parent shell env (layer 1). The
/// remaining layers are applied by the caller *around* these pairs so a later
/// layer always wins per key:
///
/// - **Layer 1** (parent shell env) is inherited by the child automatically
///   and is overridden by any key returned here. It is **also** the base
///   expansion source, so a value like `PATH=/x/bin:$PATH` prepends to the
///   inherited PATH (and does not leak parent keys into the returned pairs).
/// - **Layer 3** (CLI `--env-file`) is a future input owned by another task;
///   when added it is appended after these pairs (higher priority).
/// - **Layer 4** (`OMAKURE_RUN_ID` / `OMAKURE_SCRIPTS_DIR`) is pushed onto
///   `extra_env` *after* these pairs in
///   [`crate::run_executor::execute_with_heartbeat`], and is therefore
///   **non-overridable**: a user key of the same name from this env file
///   cannot clobber the reserved value.
///
/// Behavior change (was: prefill-only): prior to this, `.omaken/envs/*.conf`
/// only prefilled TUI schema-field defaults and never reached the spawned
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
/// folded **on top** (`.docs/env-injection-spec.md` §1).
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
/// env-injection grammar (`.docs/env-injection-spec.md` section 2).
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
        "password", "secret", "token", "key", "api", "private", "cred",
    ];
    tokens.iter().any(|token| lower.contains(token))
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

    #[rstest]
    #[case::simple_pair("HOST=localhost", vec![("HOST", "localhost")])]
    #[case::sensitive_masked("DB_PASSWORD=secret123", vec![("DB_PASSWORD", "***")])]
    #[case::api_key_masked("API_KEY=abc", vec![("API_KEY", "***")])]
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
    #[case::command_sub_literal("$(echo hi)", "$(echo hi)")]
    #[case::backtick_literal("`date`", "`date`")]
    #[case::rich_form_empty("${FOO:-x}", "")]
    #[case::reserved_visible("id=$OMAKURE_RUN_ID", "id=r-1")]
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
        assert_eq!(preview[1], ("API_KEY".to_string(), "***".to_string()));
    }
}
