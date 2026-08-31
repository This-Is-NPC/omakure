//! Multi-token bearer auth: Argon2id tokens file, scopes, and legacy env token.
//!
//! Modes:
//! - **Legacy** — `OMAKURE_API_TOKEN` as token id `legacy` with scopes `*`.
//!   Process-wide `--capability` still gates routes.
//! - **Tokens file** — `--tokens-file` / `OMAKURE_TOKENS_FILE` TOML with per-token
//!   Argon2id hashes and scopes. Process-wide `--capability` is ignored.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub const TOKEN_PREFIX: &str = "omk_live_";
pub const LEGACY_TOKEN_ID: &str = "legacy";
pub const WILDCARD_SCOPE: &str = "*";

/// Recommended Argon2id parameters (64 MiB, t=3, p=1).
const ARGON2_M_COST: u32 = 65536;
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 1;
/// Reject weaker hashes unless explicitly allowed for tests/dev.
const MIN_M_COST: u32 = 19_456; // ~19 MiB floor for containers
const MIN_T_COST: u32 = 2;
const MIN_P_COST: u32 = 1;

const PLAINTEXT_BYTES: usize = 32;
const APPEND_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(test)]
thread_local! {
    static ARGON2_VERIFY_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub token_id: String,
    pub scopes: Vec<String>,
}

impl AuthContext {
    pub fn has_scope(&self, required: &str) -> bool {
        scope_allows(&self.scopes, required)
    }
}

#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub id: String,
    pub hash: String,
    pub scopes: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
enum AuthBackend {
    Legacy {
        plaintext: String,
    },
    File {
        path: PathBuf,
        tokens: Vec<TokenRecord>,
    },
}

/// Hot-reloadable authenticator shared by the HTTP middleware.
#[derive(Clone)]
pub struct Authenticator {
    inner: Arc<RwLock<AuthBackend>>,
    reload_status: Arc<RwLock<AuthReloadStatus>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthStatus {
    pub mode: String,
    pub token_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reload_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reload_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reload_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default)]
struct AuthReloadStatus {
    last_reload_ok: Option<bool>,
    last_reload_error: Option<String>,
    last_reload_at_ms: Option<i64>,
}

impl Authenticator {
    pub fn legacy(plaintext: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(AuthBackend::Legacy {
                plaintext: plaintext.into(),
            })),
            reload_status: Arc::new(RwLock::new(AuthReloadStatus::default())),
        }
    }

    pub fn from_tokens_file(path: impl Into<PathBuf>) -> Result<Self, AuthError> {
        let path = path.into();
        let tokens = load_tokens_file(&path)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(AuthBackend::File { path, tokens })),
            reload_status: Arc::new(RwLock::new(AuthReloadStatus::default())),
        })
    }

    pub fn is_file_mode(&self) -> bool {
        matches!(
            *self.inner.read().expect("auth lock"),
            AuthBackend::File { .. }
        )
    }

    /// Authenticate a presented bearer token. Returns `None` for unknown/disabled.
    pub fn authenticate(&self, presented: &str) -> Option<AuthContext> {
        let guard = self.inner.read().expect("auth lock");
        match &*guard {
            AuthBackend::Legacy { plaintext } => {
                if constant_time_eq(presented.as_bytes(), plaintext.as_bytes()) {
                    Some(AuthContext {
                        token_id: LEGACY_TOKEN_ID.to_string(),
                        scopes: vec![WILDCARD_SCOPE.to_string()],
                    })
                } else {
                    None
                }
            }
            AuthBackend::File { tokens, .. } => authenticate_against_file(tokens, presented),
        }
    }

    /// Metadata-only auth status (no token ids, hashes, paths, or plaintext).
    pub fn status(&self) -> AuthStatus {
        let guard = self.inner.read().expect("auth lock");
        let reload = self.reload_status.read().expect("reload status lock");
        match &*guard {
            AuthBackend::Legacy { .. } => AuthStatus {
                mode: "legacy".to_string(),
                token_count: 1,
                last_reload_ok: reload.last_reload_ok,
                last_reload_error: reload.last_reload_error.clone(),
                last_reload_at_ms: reload.last_reload_at_ms,
            },
            AuthBackend::File { tokens, .. } => AuthStatus {
                mode: "tokens_file".to_string(),
                token_count: tokens.len(),
                last_reload_ok: reload.last_reload_ok,
                last_reload_error: reload.last_reload_error.clone(),
                last_reload_at_ms: reload.last_reload_at_ms,
            },
        }
    }

    /// Reload tokens from disk. On failure, keeps the last valid set and returns Err.
    pub fn reload(&self) -> Result<(), AuthError> {
        let mut guard = self.inner.write().expect("auth lock");
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        match &mut *guard {
            AuthBackend::Legacy { .. } => Ok(()),
            AuthBackend::File { path, tokens } => match load_tokens_file(path) {
                Ok(loaded) => {
                    *tokens = loaded;
                    let mut reload = self.reload_status.write().expect("reload status lock");
                    reload.last_reload_ok = Some(true);
                    reload.last_reload_error = None;
                    reload.last_reload_at_ms = Some(now_ms);
                    Ok(())
                }
                Err(err) => {
                    let mut reload = self.reload_status.write().expect("reload status lock");
                    reload.last_reload_ok = Some(false);
                    reload.last_reload_error = Some(err.status_message().to_string());
                    reload.last_reload_at_ms = Some(now_ms);
                    Err(err)
                }
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Io(String),
    Parse(String),
    DuplicateId(String),
    InvalidHash(String),
    WeakHashParams {
        id: String,
        detail: String,
    },
    EmptyId,
    EmptyScopes {
        id: String,
    },
    MissingAuth,
    InvalidLegacyToken,
    /// `auth.legacy_env_token = false` in deploy policy rejects env token.
    LegacyEnvTokenDisabled,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "tokens file I/O error: {msg}"),
            Self::Parse(msg) => write!(f, "tokens file parse error: {msg}"),
            Self::DuplicateId(id) => write!(f, "duplicate token id: {id}"),
            Self::InvalidHash(id) => write!(f, "invalid Argon2id hash for token id: {id}"),
            Self::WeakHashParams { id, detail } => {
                write!(f, "weak Argon2 params for token id {id}: {detail}")
            }
            Self::EmptyId => write!(f, "token id must not be empty"),
            Self::EmptyScopes { id } => write!(f, "token id {id} has empty scopes"),
            Self::MissingAuth => write!(
                f,
                "auth required: set OMAKURE_TOKENS_FILE/--tokens-file or OMAKURE_API_TOKEN"
            ),
            Self::InvalidLegacyToken => write!(f, "OMAKURE_API_TOKEN is invalid"),
            Self::LegacyEnvTokenDisabled => write!(
                f,
                "OMAKURE_API_TOKEN rejected: policy auth.legacy_env_token=false requires --tokens-file / OMAKURE_TOKENS_FILE"
            ),
        }
    }
}

impl std::error::Error for AuthError {}

impl AuthError {
    fn status_message(&self) -> &'static str {
        match self {
            Self::Io(_) => "tokens file I/O error",
            Self::Parse(_) => "tokens file parse error",
            Self::DuplicateId(_) => "tokens file contains duplicate token ids",
            Self::InvalidHash(_) => "tokens file contains an invalid Argon2id hash",
            Self::WeakHashParams { .. } => "tokens file contains weak Argon2 parameters",
            Self::EmptyId => "tokens file contains an empty token id",
            Self::EmptyScopes { .. } => "tokens file contains a token with empty scopes",
            Self::MissingAuth => "authentication is not configured",
            Self::InvalidLegacyToken => "legacy authentication token is invalid",
            Self::LegacyEnvTokenDisabled => "legacy authentication token is disabled",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokensFileToml {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    tokens: Vec<TokenToml>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenToml {
    id: String,
    hash: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_enabled() -> bool {
    true
}

pub fn load_tokens_file(path: &Path) -> Result<Vec<TokenRecord>, AuthError> {
    let text = fs::read_to_string(path).map_err(|e| AuthError::Io(e.to_string()))?;
    parse_tokens_toml(&text)
}

pub fn parse_tokens_toml(text: &str) -> Result<Vec<TokenRecord>, AuthError> {
    let parsed: TokensFileToml =
        toml::from_str(text).map_err(|e| AuthError::Parse(e.message().to_string()))?;
    if parsed.version != 1 {
        return Err(AuthError::Parse(format!(
            "unsupported tokens file version: {}",
            parsed.version
        )));
    }

    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(parsed.tokens.len());
    if parsed.tokens.len() > MAX_TOKENS_PER_FILE {
        return Err(AuthError::Parse(format!(
            "tokens file has {} entries (max {MAX_TOKENS_PER_FILE})",
            parsed.tokens.len()
        )));
    }
    for entry in parsed.tokens {
        let id = entry.id.trim().to_string();
        if id.is_empty() {
            return Err(AuthError::EmptyId);
        }
        if !seen.insert(id.clone()) {
            return Err(AuthError::DuplicateId(id));
        }
        if entry.scopes.is_empty() {
            return Err(AuthError::EmptyScopes { id });
        }
        validate_phc_hash(&id, &entry.hash)?;
        out.push(TokenRecord {
            id,
            hash: normalize_phc(&entry.hash),
            scopes: entry.scopes,
            enabled: entry.enabled,
        });
    }
    Ok(out)
}

fn normalize_phc(hash: &str) -> String {
    if hash.starts_with('$') {
        hash.to_string()
    } else {
        format!("${hash}")
    }
}

fn validate_phc_hash(id: &str, hash: &str) -> Result<(), AuthError> {
    let normalized = normalize_phc(hash);
    let parsed =
        PasswordHash::new(&normalized).map_err(|_| AuthError::InvalidHash(id.to_string()))?;
    if parsed.algorithm.as_str() != "argon2id" {
        return Err(AuthError::InvalidHash(id.to_string()));
    }
    let m = parsed
        .params
        .get_str("m")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let t = parsed
        .params
        .get_str("t")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let p = parsed
        .params
        .get_str("p")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    if m < MIN_M_COST {
        return Err(AuthError::WeakHashParams {
            id: id.to_string(),
            detail: format!("m={m} below minimum {MIN_M_COST}"),
        });
    }
    if t < MIN_T_COST {
        return Err(AuthError::WeakHashParams {
            id: id.to_string(),
            detail: format!("t={t} below minimum {MIN_T_COST}"),
        });
    }
    if p < MIN_P_COST {
        return Err(AuthError::WeakHashParams {
            id: id.to_string(),
            detail: format!("p={p} below minimum {MIN_P_COST}"),
        });
    }
    Ok(())
}

fn authenticate_against_file(tokens: &[TokenRecord], presented: &str) -> Option<AuthContext> {
    if let Some(id) = token_selector(presented) {
        return tokens
            .iter()
            .find(|token| token.enabled && token.id == id)
            .filter(|token| verify_argon2(&token.hash, presented))
            .map(auth_context);
    }
    if !is_legacy_token_shape(presented) {
        // Neither the new selector format nor the pre-upgrade shape: cannot be
        // a real token, so skip Argon2 entirely (keeps the auth-flood bound
        // effective against arbitrary bearer strings).
        return None;
    }
    // Tokens generated before the token_selector upgrade carry no embedded id
    // (`omk_live_<64 hex>`, not `omk_live_<hex id>_<64 hex>`), so there is no
    // id to look up by. Fall back to checking every enabled token's hash, with
    // no early exit, matching the pre-upgrade behavior these tokens were
    // issued under (avoids leaking a token's position via timing).
    let mut matched: Option<AuthContext> = None;
    for token in tokens.iter().filter(|t| t.enabled) {
        if verify_argon2(&token.hash, presented) {
            matched = Some(auth_context(token));
        }
    }
    matched
}

/// Whether `presented` has the pre-token_selector shape: `TOKEN_PREFIX` plus
/// exactly `PLAINTEXT_BYTES * 2` hex characters and no embedded id/underscore.
fn is_legacy_token_shape(presented: &str) -> bool {
    presented.strip_prefix(TOKEN_PREFIX).is_some_and(|rest| {
        rest.len() == PLAINTEXT_BYTES * 2 && rest.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

fn auth_context(token: &TokenRecord) -> AuthContext {
    AuthContext {
        token_id: token.id.clone(),
        scopes: token.scopes.clone(),
    }
}

fn token_selector(presented: &str) -> Option<String> {
    let remainder = presented.strip_prefix(TOKEN_PREFIX)?;
    let (encoded_id, secret) = remainder.split_once('_')?;
    if encoded_id.is_empty()
        || secret.len() != PLAINTEXT_BYTES * 2
        || !secret.bytes().all(|b| b.is_ascii_hexdigit())
        || encoded_id.len() % 2 != 0
    {
        return None;
    }
    let mut bytes = Vec::with_capacity(encoded_id.len() / 2);
    for pair in encoded_id.as_bytes().chunks_exact(2) {
        let pair = std::str::from_utf8(pair).ok()?;
        bytes.push(u8::from_str_radix(pair, 16).ok()?);
    }
    String::from_utf8(bytes).ok()
}

fn verify_argon2(phc: &str, presented: &str) -> bool {
    #[cfg(test)]
    ARGON2_VERIFY_COUNT.with(|count| count.set(count.get() + 1));
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(presented.as_bytes(), &parsed)
        .is_ok()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Digest both sides so length mismatches do not short-circuit (length oracle).
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;
    let ha = Sha256::digest(a);
    let hb = Sha256::digest(b);
    ha.ct_eq(&hb).into()
}

/// Whether `granted` scopes satisfy `required`.
///
/// Supports `*`, exact match, env name aliases (`env:read` ↔ `envs:read`), and
/// **one-way** coarse→fine coverage (`runs:write` covers `runs:enqueue`, but
/// `runs:enqueue` does **not** satisfy a `runs:write` check). Fine scopes must
/// never escalate to coarser write privileges.
pub fn scope_allows(granted: &[String], required: &str) -> bool {
    if granted.iter().any(|s| s == WILDCARD_SCOPE) {
        return true;
    }
    for g in granted {
        if scopes_match(g, required) {
            return true;
        }
    }
    false
}

fn scopes_match(granted: &str, required: &str) -> bool {
    if normalize_scope(granted) == normalize_scope(required) {
        return true;
    }
    // Coarse grants cover finer required actions only (never the reverse).
    matches!(
        (granted, required),
        (
            "runs:write",
            "runs:enqueue" | "runs:cancel" | "runs:dead-letter" | "runs:write"
        ) | (
            "batteries:write",
            "batteries:add"
                | "batteries:sync"
                | "batteries:install"
                | "batteries:remove"
                | "batteries:write",
        ) | (
            "config:read",
            "config:read" | "doctor:read" | "workspace:read"
        ) | ("scripts:read", "scripts:read" | "search:read")
    )
}

fn normalize_scope(scope: &str) -> &str {
    match scope {
        "env:read" | "envs:read" => "envs:read",
        "env:write" | "envs:write" => "envs:write",
        "env:activate" | "envs:activate" => "envs:activate",
        "env:use" | "envs:use" => "envs:use",
        other => other,
    }
}

/// Resolve auth configuration from CLI/env (legacy env token allowed).
#[allow(dead_code)] // public helper; api uses resolve_authenticator_with_legacy
pub fn resolve_authenticator(
    tokens_file: Option<&Path>,
    tokens_file_env: Option<&str>,
) -> Result<Authenticator, AuthError> {
    resolve_authenticator_with_legacy(tokens_file, tokens_file_env, true)
}

pub fn resolve_authenticator_with_legacy(
    tokens_file: Option<&Path>,
    tokens_file_env: Option<&str>,
    allow_legacy_env_token: bool,
) -> Result<Authenticator, AuthError> {
    let path = tokens_file
        .map(Path::to_path_buf)
        .or_else(|| tokens_file_env.map(PathBuf::from));
    if let Some(path) = path {
        return Authenticator::from_tokens_file(path);
    }
    if !allow_legacy_env_token {
        // Policy disabled the legacy env token; reject regardless of whether one
        // is present so the operator gets a consistent, actionable error.
        return Err(AuthError::LegacyEnvTokenDisabled);
    }
    let token = std::env::var("OMAKURE_API_TOKEN").map_err(|_| AuthError::MissingAuth)?;
    validate_legacy_token(&token)?;
    Ok(Authenticator::legacy(token.trim()))
}

pub fn validate_legacy_token(token: &str) -> Result<(), AuthError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(AuthError::MissingAuth);
    }
    let lower = trimmed.to_ascii_lowercase();
    let known_defaults = [
        "changeme",
        "change-me",
        "default",
        "password",
        "secret",
        "token",
        "omakure",
    ];
    if trimmed.len() < 32 || known_defaults.contains(&lower.as_str()) {
        return Err(AuthError::InvalidLegacyToken);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct GeneratedToken {
    pub id: String,
    pub token: String,
    pub hash: String,
    pub scopes: Vec<String>,
    pub tokens_file_entry: String,
}

pub fn generate_token(id: &str, scopes: &[String]) -> Result<GeneratedToken, AuthError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(AuthError::EmptyId);
    }
    if scopes.is_empty() {
        return Err(AuthError::EmptyScopes { id: id.to_string() });
    }
    let plaintext = selector_token_plaintext(id);
    let hash = hash_token(&plaintext)?;
    let entry = format_toml_entry(id, &hash, scopes);
    Ok(GeneratedToken {
        id: id.to_string(),
        token: plaintext,
        hash,
        scopes: scopes.to_vec(),
        tokens_file_entry: entry,
    })
}

fn selector_token_plaintext(id: &str) -> String {
    let mut bytes = [0u8; PLAINTEXT_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let mut encoded =
        String::with_capacity(TOKEN_PREFIX.len() + id.len() * 2 + 1 + bytes.len() * 2);
    encoded.push_str(TOKEN_PREFIX);
    for b in id.as_bytes() {
        encoded.push_str(&format!("{b:02x}"));
    }
    encoded.push('_');
    for b in bytes {
        encoded.push_str(&format!("{b:02x}"));
    }
    encoded
}

#[cfg(test)]
pub fn test_token_plaintext(id: &str) -> String {
    selector_token_plaintext(id)
}

pub fn hash_token(plaintext: &str) -> Result<String, AuthError> {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, None)
        .map_err(|e| AuthError::Parse(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2
        .hash_password(plaintext.as_bytes(), &salt)
        .map_err(|e| AuthError::Parse(e.to_string()))?;
    Ok(hash.to_string())
}

pub fn format_toml_entry(id: &str, hash: &str, scopes: &[String]) -> String {
    let mut out = String::new();
    out.push_str("[[tokens]]\n");
    out.push_str(&format!("id = \"{}\"\n", escape_toml_str(id)));
    out.push_str(&format!("hash = \"{}\"\n", escape_toml_str(hash)));
    out.push_str("scopes = [\n");
    for scope in scopes {
        out.push_str(&format!("  \"{}\",\n", escape_toml_str(scope)));
    }
    out.push_str("]\n");
    out.push_str("enabled = true\n");
    out
}

fn escape_toml_str(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub const MAX_TOKENS_PER_FILE: usize = 64;

pub fn append_token_entry(path: &Path, id: &str, entry: &str) -> Result<(), AuthError> {
    use std::io::Write;
    let _lock = AppendLock::acquire(path)?;
    // Validate uniqueness before mutating the file.
    if path.exists() {
        let existing = load_tokens_file(path)?;
        if existing.len() >= MAX_TOKENS_PER_FILE {
            return Err(AuthError::Parse(format!(
                "tokens file already has {} entries (max {MAX_TOKENS_PER_FILE})",
                existing.len()
            )));
        }
        if existing.iter().any(|t| t.id == id) {
            return Err(AuthError::DuplicateId(id.to_string()));
        }
    }
    let mut staged = if path.exists() {
        fs::read_to_string(path).map_err(|e| AuthError::Io(e.to_string()))?
    } else {
        "version = 1\n\n".to_string()
    };
    if !staged.ends_with('\n') {
        staged.push('\n');
    }
    if !staged.ends_with("\n\n") && staged.trim_end() != "version = 1" {
        staged.push('\n');
    }
    staged.push_str(entry);
    if !entry.ends_with('\n') {
        staged.push('\n');
    }
    // Re-parse staged content before replace so we never leave a broken file.
    let _ = parse_tokens_toml(&staged)?;
    // Read under the append lock, so the mode/ownership carried forward is the
    // one belonging to the file this append is actually replacing.
    #[cfg(unix)]
    let replaced_metadata = fs::metadata(path).ok();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    // Randomize the tmp suffix so two concurrent appends in the same process
    // never collide on the path, and so the path is unpredictable (an attacker
    // cannot pre-plant a file/symlink at a guessable tmp name).
    let mut tmp_rand = [0u8; 8];
    OsRng.fill_bytes(&mut tmp_rand);
    let tmp_rand: String = tmp_rand.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tokens.toml"),
        std::process::id(),
        tmp_rand
    ));
    {
        let mut opts = fs::OpenOptions::new();
        // O_EXCL (`create_new`) so a pre-planted file/symlink at the tmp path is
        // never followed or truncated. Least-privilege 0600 since the tokens file
        // holds credential hashes.
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = match opts.open(&tmp) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // Stale tmp (e.g. a crashed prior append with the same pid).
                // `remove_file` unlinks the entry itself — it does not follow a
                // symlink target — then O_EXCL re-create refuses any re-plant.
                fs::remove_file(&tmp).map_err(|e| AuthError::Io(e.to_string()))?;
                opts.open(&tmp).map_err(|e| AuthError::Io(e.to_string()))?
            }
            Err(err) => return Err(AuthError::Io(err.to_string())),
        };
        file.write_all(staged.as_bytes())
            .map_err(|e| AuthError::Io(e.to_string()))?;
        file.sync_all().map_err(|e| AuthError::Io(e.to_string()))?;
    }
    // Carry the destination's existing mode and ownership onto the replacement.
    // The staged file is deliberately created 0600 and owned by whoever runs the
    // append, but the installed tokens file is `root:omakure 0640` so the
    // unprivileged service user can read it. Renaming a fresh 0600 root-owned
    // file over it would lock the service out of its own credentials — and only
    // at the *next* restart, long after the append that caused it.
    #[cfg(unix)]
    if let Some(existing) = replaced_metadata.as_ref() {
        preserve_ownership_and_mode(&tmp, existing)?;
    }
    #[cfg(windows)]
    if path.exists() {
        // ReplaceFileW atomically replaces the destination while preserving
        // its metadata and security descriptor. Keep the sidecar lock held
        // for the whole operation so token writers remain serialized.
        replace_existing_windows(&tmp, path)?;
    } else {
        fs::rename(&tmp, path).map_err(|e| AuthError::Io(e.to_string()))?;
    }
    #[cfg(not(windows))]
    fs::rename(&tmp, path).map_err(|e| AuthError::Io(e.to_string()))?;
    Ok(())
}

/// Atomically install a staged token file over an existing destination on
/// Windows. `ReplaceFileW` preserves the destination's metadata and security
/// descriptor; unlike remove-then-rename there is no observable delete gap.
#[cfg(windows)]
fn replace_existing_windows(tmp: &Path, destination: &Path) -> Result<(), AuthError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let replacement = tmp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both paths are NUL-terminated UTF-16 strings that remain alive
    // for the duration of the synchronous API call. The null backup and
    // exclusion/preserve pointers request no backup and default behavior.
    let result = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        Err(AuthError::Io(std::io::Error::last_os_error().to_string()))
    } else {
        Ok(())
    }
}

/// Apply `existing`'s mode and ownership to the staged replacement at `tmp`.
///
/// Ownership is only changed when it actually differs, so an unprivileged
/// operator appending to a file they already own never needs `CAP_CHOWN`. When
/// it does differ and the chown is refused, that is reported rather than
/// swallowed: silently completing the append is what leaves a node unable to
/// read its own tokens file.
#[cfg(unix)]
fn preserve_ownership_and_mode(tmp: &Path, existing: &fs::Metadata) -> Result<(), AuthError> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let staged = fs::metadata(tmp).map_err(|e| AuthError::Io(e.to_string()))?;
    if staged.uid() != existing.uid() || staged.gid() != existing.gid() {
        let raw = std::ffi::CString::new(tmp.as_os_str().as_bytes())
            .map_err(|e| AuthError::Io(e.to_string()))?;
        // SAFETY: `raw` is a NUL-terminated path we own for the call's duration.
        let rc = unsafe { libc::chown(raw.as_ptr(), existing.uid(), existing.gid()) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(AuthError::Io(format!(
                "could not preserve tokens file ownership {}:{} on {}: {err}",
                existing.uid(),
                existing.gid(),
                tmp.display()
            )));
        }
    }
    fs::set_permissions(tmp, fs::Permissions::from_mode(existing.mode() & 0o7777))
        .map_err(|e| AuthError::Io(e.to_string()))?;
    Ok(())
}

struct AppendLock {
    file: fs::File,
}

impl AppendLock {
    fn acquire(tokens_path: &Path) -> Result<Self, AuthError> {
        let parent = tokens_path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = tokens_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tokens.toml");
        let path = parent.join(format!(".{file_name}.append.lock"));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|err| AuthError::Io(err.to_string()))?;
        let start = std::time::Instant::now();
        loop {
            match fs2::FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() >= APPEND_LOCK_TIMEOUT {
                        return Err(AuthError::Io(
                            "timed out waiting for another token-file append".to_string(),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(err) => return Err(AuthError::Io(err.to_string())),
            }
        }
    }
}

impl Drop for AppendLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

/// Install a SIGHUP handler that reloads the authenticator (Unix only).
/// Failed reloads keep the last valid set.
#[cfg(unix)]
pub fn install_sighup_reload(auth: Authenticator) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }

    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_hook = Arc::clone(&flag);
    let _ = signal_hook::flag::register(signal_hook::consts::SIGHUP, flag_for_hook);

    thread::spawn(move || loop {
        if flag.swap(false, Ordering::SeqCst) {
            match auth.reload() {
                Ok(()) => eprintln!("omakure: reloaded tokens file"),
                Err(err) => {
                    eprintln!(
                        "omakure: tokens reload failed; keeping last valid set ({})",
                        err.status_message()
                    );
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    });
}

#[cfg(not(unix))]
pub fn install_sighup_reload(_auth: Authenticator) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_rejects_duplicate_ids() {
        let hash = hash_token(&test_token_plaintext("ci")).unwrap();
        let toml = format!(
            r#"
version = 1
[[tokens]]
id = "ci"
hash = "{hash}"
scopes = ["runs:read"]
[[tokens]]
id = "ci"
hash = "{hash}"
scopes = ["runs:enqueue"]
"#
        );
        let err = parse_tokens_toml(&toml).unwrap_err();
        assert!(matches!(err, AuthError::DuplicateId(id) if id == "ci"));
    }

    #[test]
    fn parse_rejects_empty_scopes() {
        let hash = hash_token(&test_token_plaintext("ci")).unwrap();
        let toml = format!(
            r#"
version = 1
[[tokens]]
id = "ci"
hash = "{hash}"
scopes = []
"#
        );
        assert!(matches!(
            parse_tokens_toml(&toml),
            Err(AuthError::EmptyScopes { .. })
        ));
    }

    #[test]
    fn parse_rejects_non_argon2id() {
        let toml = r#"
version = 1
[[tokens]]
id = "ci"
hash = "$bcrypt$v=0$not-real"
scopes = ["*"]
"#;
        assert!(matches!(
            parse_tokens_toml(toml),
            Err(AuthError::InvalidHash(_))
        ));
    }

    #[test]
    fn parse_rejects_unknown_top_level_and_token_fields() {
        let hash = hash_token(&test_token_plaintext("ci")).unwrap();
        let top_level = format!(
            "version = 1\nunknown = true\n[[tokens]]\nid = \"ci\"\nhash = \"{hash}\"\nscopes = [\"*\"]\n"
        );
        let entry = format!(
            "version = 1\n[[tokens]]\nid = \"ci\"\nhash = \"{hash}\"\nscopes = [\"*\"]\nunknown = true\n"
        );

        for (field, text) in [("top-level", top_level), ("entry", entry)] {
            let err = parse_tokens_toml(&text).unwrap_err();
            assert!(
                matches!(err, AuthError::Parse(_)),
                "accepted unknown {field} field"
            );
            assert!(err.to_string().contains("unknown"));
        }
    }

    #[test]
    fn disabled_token_does_not_authenticate() {
        let plaintext = test_token_plaintext("disabled");
        let hash = hash_token(&plaintext).unwrap();
        let toml = format!(
            r#"
version = 1
[[tokens]]
id = "disabled"
hash = "{hash}"
scopes = ["*"]
enabled = false
"#
        );
        let tokens = parse_tokens_toml(&toml).unwrap();
        assert!(authenticate_against_file(&tokens, &plaintext).is_none());
    }

    #[test]
    fn enabled_token_authenticates_with_scopes() {
        let plaintext = test_token_plaintext("ci-deployer");
        let hash = hash_token(&plaintext).unwrap();
        let toml = format!(
            r#"
version = 1
[[tokens]]
id = "ci-deployer"
hash = "{hash}"
scopes = ["runs:enqueue", "runs:read"]
enabled = true
"#
        );
        let tokens = parse_tokens_toml(&toml).unwrap();
        let ctx = authenticate_against_file(&tokens, &plaintext).unwrap();
        assert_eq!(ctx.token_id, "ci-deployer");
        assert!(ctx.has_scope("runs:read"));
        assert!(ctx.has_scope("runs:enqueue"));
        assert!(!ctx.has_scope("batteries:add"));
    }

    #[test]
    fn authenticate_matches_correct_token_among_many() {
        // Early-exit refactor must still select the matching token regardless of
        // position, and reject a plaintext that matches none of them.
        let mut toml = String::from("version = 1\n");
        let mut plaintexts = Vec::new();
        for id in ["a", "b", "target", "d"] {
            let plaintext = test_token_plaintext(id);
            let hash = hash_token(&plaintext).unwrap();
            toml.push_str(&format!(
                "[[tokens]]\nid = \"{id}\"\nhash = \"{hash}\"\nscopes = [\"runs:read\"]\nenabled = true\n"
            ));
            plaintexts.push((id, plaintext));
        }
        let tokens = parse_tokens_toml(&toml).unwrap();
        let (_, target_plaintext) = plaintexts.iter().find(|(id, _)| *id == "target").unwrap();
        let ctx = authenticate_against_file(&tokens, target_plaintext).unwrap();
        assert_eq!(ctx.token_id, "target");
        assert!(authenticate_against_file(&tokens, "omk_live_deadbeef-not-a-real-token").is_none());
    }

    #[test]
    fn selector_authenticates_with_one_argon2_verification() {
        let generated = generate_token("target", &["runs:read".into()]).unwrap();
        let mut tokens = (0..MAX_TOKENS_PER_FILE)
            .map(|index| TokenRecord {
                id: format!("other-{index}"),
                hash: generated.hash.clone(),
                scopes: vec!["runs:read".into()],
                enabled: true,
            })
            .collect::<Vec<_>>();
        tokens.push(TokenRecord {
            id: generated.id.clone(),
            hash: generated.hash.clone(),
            scopes: generated.scopes.clone(),
            enabled: true,
        });

        ARGON2_VERIFY_COUNT.with(|count| count.set(0));
        let ctx = authenticate_against_file(&tokens, &generated.token).unwrap();
        assert_eq!(ctx.token_id, "target");
        ARGON2_VERIFY_COUNT.with(|count| assert_eq!(count.get(), 1));

        let unknown = selector_token_plaintext("missing");
        ARGON2_VERIFY_COUNT.with(|count| count.set(0));
        assert!(authenticate_against_file(&tokens, &unknown).is_none());
        ARGON2_VERIFY_COUNT.with(|count| assert_eq!(count.get(), 0));
    }

    #[test]
    fn bearer_without_selector_does_no_argon2_work() {
        let plaintext = test_token_plaintext("configured");
        let hash = hash_token(&plaintext).unwrap();
        let tokens = (0..MAX_TOKENS_PER_FILE)
            .map(|index| TokenRecord {
                id: format!("legacy-{index}"),
                hash: hash.clone(),
                scopes: vec!["runs:read".into()],
                enabled: true,
            })
            .collect::<Vec<_>>();

        // Not the token_selector shape and not the pre-upgrade legacy shape
        // (no TOKEN_PREFIX at all) — must never trigger Argon2 work.
        ARGON2_VERIFY_COUNT.with(|count| count.set(0));
        assert!(authenticate_against_file(&tokens, "invalid legacy bearer").is_none());
        ARGON2_VERIFY_COUNT.with(|count| assert_eq!(count.get(), 0));
    }

    #[test]
    fn pre_upgrade_legacy_shaped_token_still_authenticates() {
        // Tokens minted before the token_selector upgrade (1726c8c) are bare
        // `TOKEN_PREFIX + 64 hex chars`, with no embedded id. Deploying the
        // selector upgrade must not lock out tokens already distributed under
        // the old format.
        let legacy_plaintext = format!("{TOKEN_PREFIX}{}", "ab".repeat(PLAINTEXT_BYTES));
        let hash = hash_token(&legacy_plaintext).unwrap();
        let other_hash = hash_token(&test_token_plaintext("other")).unwrap();
        // Keep this compatibility test focused on the legacy shape. The
        // selector tests cover the maximum file size; scanning 64 Argon2
        // hashes here makes the suite needlessly expensive on slow hosts.
        let mut tokens = (0..1)
            .map(|index| TokenRecord {
                id: format!("other-{index}"),
                hash: other_hash.clone(),
                scopes: vec!["runs:read".into()],
                enabled: true,
            })
            .collect::<Vec<_>>();
        tokens.push(TokenRecord {
            id: "legacy-holder".into(),
            hash,
            scopes: vec!["runs:read".into()],
            enabled: true,
        });

        let ctx = authenticate_against_file(&tokens, &legacy_plaintext).unwrap();
        assert_eq!(ctx.token_id, "legacy-holder");

        // A legacy-shaped guess that matches no hash is rejected, not panicked.
        let wrong = format!("{TOKEN_PREFIX}{}", "cd".repeat(PLAINTEXT_BYTES));
        assert!(authenticate_against_file(&tokens, &wrong).is_none());
    }

    #[test]
    fn wildcard_scope_allows_all() {
        let ctx = AuthContext {
            token_id: "admin".into(),
            scopes: vec!["*".into()],
        };
        assert!(ctx.has_scope("runs:enqueue"));
        assert!(ctx.has_scope("admin:status"));
    }

    #[test]
    fn env_scope_aliases() {
        let ctx = AuthContext {
            token_id: "t".into(),
            scopes: vec!["envs:read".into()],
        };
        assert!(ctx.has_scope("env:read"));
        assert!(ctx.has_scope("envs:read"));
        let ctx2 = AuthContext {
            token_id: "t".into(),
            scopes: vec!["env:write".into()],
        };
        assert!(ctx2.has_scope("envs:write"));
    }

    #[test]
    fn runs_write_covers_finer_plan_scopes() {
        let ctx = AuthContext {
            token_id: "t".into(),
            scopes: vec!["runs:write".into()],
        };
        assert!(ctx.has_scope("runs:enqueue"));
        assert!(ctx.has_scope("runs:cancel"));
        assert!(ctx.has_scope("runs:dead-letter"));
    }

    #[test]
    fn fine_scopes_do_not_satisfy_coarse_write_checks() {
        let runs = AuthContext {
            token_id: "t".into(),
            scopes: vec!["runs:enqueue".into()],
        };
        assert!(runs.has_scope("runs:enqueue"));
        assert!(!runs.has_scope("runs:write"));
        assert!(!runs.has_scope("runs:cancel"));

        let batteries = AuthContext {
            token_id: "t".into(),
            scopes: vec!["batteries:add".into()],
        };
        assert!(batteries.has_scope("batteries:add"));
        assert!(!batteries.has_scope("batteries:write"));
        assert!(!batteries.has_scope("batteries:sync"));
        assert!(!batteries.has_scope("batteries:install"));
        assert!(!batteries.has_scope("batteries:remove"));
    }

    #[test]
    fn batteries_write_covers_finer_battery_scopes() {
        let ctx = AuthContext {
            token_id: "t".into(),
            scopes: vec!["batteries:write".into()],
        };
        assert!(ctx.has_scope("batteries:add"));
        assert!(ctx.has_scope("batteries:sync"));
        assert!(ctx.has_scope("batteries:install"));
        assert!(ctx.has_scope("batteries:remove"));
    }

    #[test]
    fn legacy_authenticator_uses_legacy_id_and_wildcard() {
        let token = "0123456789abcdef0123456789abcdef";
        let auth = Authenticator::legacy(token);
        let ctx = auth.authenticate(token).unwrap();
        assert_eq!(ctx.token_id, LEGACY_TOKEN_ID);
        assert!(ctx.has_scope("anything"));
        assert!(auth
            .authenticate("wrong-token-value-that-is-long-enough!!")
            .is_none());
    }

    #[test]
    fn generate_token_uses_prefix_and_verifiable_hash() {
        let gen = generate_token("ci", &["runs:read".into(), "scripts:read".into()]).unwrap();
        assert!(gen.token.starts_with(TOKEN_PREFIX));
        assert!(gen.hash.contains("argon2id"));
        assert!(gen.tokens_file_entry.contains("id = \"ci\""));
        assert!(verify_argon2(&gen.hash, &gen.token));
        assert!(!gen.tokens_file_entry.contains(&gen.token));
    }

    #[test]
    fn reload_keeps_last_valid_set_on_failure() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let plaintext = test_token_plaintext("ok");
        let hash = hash_token(&plaintext).unwrap();
        fs::write(
            &path,
            format!(
                r#"
version = 1
[[tokens]]
id = "ok"
hash = "{hash}"
scopes = ["*"]
enabled = true
"#
            ),
        )
        .unwrap();
        let auth = Authenticator::from_tokens_file(&path).unwrap();
        assert!(auth.authenticate(&plaintext).is_some());

        fs::write(&path, "this is not valid toml [[[").unwrap();
        assert!(auth.reload().is_err());
        assert!(auth.authenticate(&plaintext).is_some());
    }

    #[test]
    fn status_surfaces_reload_failure_without_secrets_or_token_ids() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let plaintext = test_token_plaintext("ok");
        let hash = hash_token(&plaintext).unwrap();
        fs::write(
            &path,
            format!(
                r#"
version = 1
[[tokens]]
id = "ok"
hash = "{hash}"
scopes = ["admin:status"]
enabled = true
"#
            ),
        )
        .unwrap();
        let auth = Authenticator::from_tokens_file(&path).unwrap();
        let before = auth.status();
        assert_eq!(before.mode, "tokens_file");
        assert_eq!(before.token_count, 1);
        assert_eq!(before.last_reload_ok, None);
        assert!(before.last_reload_error.is_none());
        let serialized = serde_json::to_string(&before).unwrap();
        assert!(!serialized.contains(&plaintext));
        // Token id must never appear; avoid matching substrings of field names.
        assert!(!serialized.contains("\"id\""));
        assert!(!serialized.contains(path.to_string_lossy().as_ref()));

        let source_canary = "RAW_SOURCE_SECRET_CANARY_7f83";
        fs::write(
            &path,
            format!("version = 1\nsecret_canary = \"{source_canary}\"\n"),
        )
        .unwrap();
        let local_error = auth.reload().unwrap_err().to_string();
        assert!(local_error.contains("secret_canary"));
        assert!(!local_error.contains(source_canary));
        let after = auth.status();
        assert_eq!(after.last_reload_ok, Some(false));
        assert!(after
            .last_reload_error
            .as_deref()
            .is_some_and(|e| !e.is_empty()));
        assert!(after.last_reload_at_ms.is_some());
        assert_eq!(after.token_count, 1);
        assert!(auth.authenticate(&plaintext).is_some());
        let status_json = serde_json::to_string(&after).unwrap();
        assert!(!status_json.contains(source_canary));
        assert!(!status_json.contains("secret_canary"));

        // Restore a valid file and confirm success is surfaced.
        fs::write(
            &path,
            format!(
                r#"
version = 1
[[tokens]]
id = "ok"
hash = "{hash}"
scopes = ["admin:status"]
enabled = true
"#
            ),
        )
        .unwrap();
        auth.reload().unwrap();
        let ok = auth.status();
        assert_eq!(ok.last_reload_ok, Some(true));
        assert!(ok.last_reload_error.is_none());
    }

    #[test]
    fn legacy_status_reports_mode_without_token_material() {
        let token = "0123456789abcdef0123456789abcdef";
        let auth = Authenticator::legacy(token);
        let status = auth.status();
        assert_eq!(status.mode, "legacy");
        assert_eq!(status.token_count, 1);
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains(token));
    }


    /// Existing token stores must be replaced in place on Windows rather than
    /// removed before the staged file is installed.
    #[cfg(windows)]
    #[test]
    fn windows_replaces_existing_token_store_atomically() {
        let dir = TempDir::new().unwrap();
        let destination = dir.path().join("tokens.toml");
        let replacement = dir.path().join("tokens.toml.tmp");
        fs::write(&destination, "old").unwrap();
        fs::write(&replacement, "new").unwrap();

        replace_existing_windows(&replacement, &destination).unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "new");
        assert!(!replacement.exists(), "staged file must be consumed");
    }
    /// Appending a token must not narrow the tokens file's permissions.
    ///
    /// The installer creates `/etc/omakure/tokens.toml` as `root:omakure 0640`
    /// precisely so the unprivileged node service can read it. An append that
    /// replaced the file with a fresh 0600 one made the service fail to start
    /// with `tokens file I/O error: Permission denied` — at the next restart,
    /// not at append time, so the outage looked unrelated to adding a token.
    #[cfg(unix)]
    #[test]
    fn append_token_entry_preserves_the_existing_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let first = generate_token("first", &["runs:read".into()]).unwrap();
        append_token_entry(&path, &first.id, &first.tokens_file_entry).unwrap();
        // Stand in for the installer's group-readable install mode.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let second = generate_token("second", &["runs:read".into()]).unwrap();
        append_token_entry(&path, &second.id, &second.tokens_file_entry).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(
            mode, 0o640,
            "append replaced the tokens file with mode {mode:o}, dropping the \
             group read bit the node service needs to read its own credentials"
        );
        assert_eq!(load_tokens_file(&path).unwrap().len(), 2);
    }

    #[test]
    fn append_token_entry_writes_parseable_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let gen = generate_token("a", &["runs:read".into()]).unwrap();
        append_token_entry(&path, &gen.id, &gen.tokens_file_entry).unwrap();
        let tokens = load_tokens_file(&path).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].id, "a");
    }

    #[test]
    fn append_recovers_when_advisory_lock_file_survives_prior_process() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        fs::write(dir.path().join(".tokens.toml.append.lock"), "stale").unwrap();
        let generated = generate_token("a", &["runs:read".into()]).unwrap();

        append_token_entry(&path, &generated.id, &generated.tokens_file_entry).unwrap();

        assert_eq!(load_tokens_file(&path).unwrap().len(), 1);
    }

    #[test]
    fn append_token_entry_process_worker() {
        let Ok(path) = std::env::var("OMAKURE_APPEND_TEST_PATH") else {
            return;
        };
        let id = std::env::var("OMAKURE_APPEND_TEST_ID").unwrap();
        let entry = std::env::var("OMAKURE_APPEND_TEST_ENTRY").unwrap();
        append_token_entry(Path::new(&path), &id, &entry).unwrap();
    }

    #[test]
    fn concurrent_process_appends_do_not_lose_updates() {
        use std::process::Command;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let hash = hash_token(&test_token_plaintext("process")).unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut children = Vec::new();

        for index in 0..6 {
            let id = format!("process-{index}");
            let entry = format_toml_entry(&id, &hash, &["runs:read".into()]);
            children.push(
                Command::new(&executable)
                    .args([
                        "--exact",
                        "auth::tests::append_token_entry_process_worker",
                        "--test-threads=1",
                    ])
                    .env("OMAKURE_APPEND_TEST_PATH", &path)
                    .env("OMAKURE_APPEND_TEST_ID", &id)
                    .env("OMAKURE_APPEND_TEST_ENTRY", entry)
                    .spawn()
                    .unwrap(),
            );
        }

        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let tokens = load_tokens_file(&path).unwrap();
        assert_eq!(tokens.len(), 6);
        for index in 0..6 {
            assert!(tokens
                .iter()
                .any(|token| token.id == format!("process-{index}")));
        }
    }

    #[test]
    #[cfg(unix)]
    fn append_token_entry_does_not_clobber_symlink_at_guessable_tmp_path() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        // Plant a symlink at the pid-guessable tmp prefix pointing at a victim
        // file. The real tmp now carries a random suffix, so this planted path is
        // never the one opened; combined with O_EXCL, the victim is safe.
        let guessable = dir
            .path()
            .join(format!(".tokens.toml.tmp-{}", std::process::id()));
        let victim = dir.path().join("victim.txt");
        fs::write(&victim, "do-not-clobber").unwrap();
        symlink(&victim, &guessable).unwrap();

        let gen = generate_token("a", &["runs:read".into()]).unwrap();
        append_token_entry(&path, &gen.id, &gen.tokens_file_entry).unwrap();

        // Victim survives untouched; tokens file is created and parseable.
        assert_eq!(fs::read_to_string(&victim).unwrap(), "do-not-clobber");
        assert_eq!(load_tokens_file(&path).unwrap().len(), 1);
    }

    #[test]
    #[cfg(unix)]
    fn append_token_entry_writes_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let gen = generate_token("a", &["runs:read".into()]).unwrap();
        append_token_entry(&path, &gen.id, &gen.tokens_file_entry).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "tokens file must be owner-only, got {mode:o}");
    }

    #[test]
    fn append_token_entry_rejects_duplicate_id_without_corrupting_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let a = generate_token("dup", &["runs:read".into()]).unwrap();
        append_token_entry(&path, &a.id, &a.tokens_file_entry).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let b = generate_token("dup", &["scripts:read".into()]).unwrap();
        let err = append_token_entry(&path, &b.id, &b.tokens_file_entry).unwrap_err();
        assert!(matches!(err, AuthError::DuplicateId(_)));
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert_eq!(load_tokens_file(&path).unwrap().len(), 1);
    }

    #[test]
    fn constant_time_eq_is_length_independent() {
        assert!(!constant_time_eq(b"short", b"a-much-longer-value"));
        assert!(constant_time_eq(b"same-bytes", b"same-bytes"));
    }

    #[test]
    fn validate_legacy_token_rejects_short_and_defaults() {
        assert!(validate_legacy_token("short").is_err());
        assert!(validate_legacy_token("changeme").is_err());
        assert!(validate_legacy_token("0123456789abcdef0123456789abcdef").is_ok());
    }
}
