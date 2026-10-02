use super::bearer::authenticate_against_file;
use super::file::load_tokens_file;
use super::scope::scope_allows;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

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
struct TokenStore {
    #[cfg(any(unix, test))]
    path: PathBuf,
    tokens: Vec<TokenRecord>,
}

/// Hot-reloadable authenticator shared by the HTTP middleware.
#[derive(Clone)]
pub struct Authenticator {
    inner: Arc<RwLock<TokenStore>>,
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
    pub fn from_tokens_file(path: impl Into<PathBuf>) -> Result<Self, AuthError> {
        let path = path.into();
        let tokens = load_tokens_file(&path)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(TokenStore {
                #[cfg(any(unix, test))]
                path,
                tokens,
            })),
            reload_status: Arc::new(RwLock::new(AuthReloadStatus::default())),
        })
    }

    /// Authenticate a presented bearer token. Returns `None` for unknown/disabled.
    pub fn authenticate(&self, presented: &str) -> Option<AuthContext> {
        let guard = self.inner.read().expect("auth lock");
        authenticate_against_file(&guard.tokens, presented)
    }

    /// Metadata-only auth status (no token ids, hashes, paths, or plaintext).
    pub fn status(&self) -> AuthStatus {
        let guard = self.inner.read().expect("auth lock");
        let reload = self.reload_status.read().expect("reload status lock");
        AuthStatus {
            mode: "tokens_file".to_string(),
            token_count: guard.tokens.len(),
            last_reload_ok: reload.last_reload_ok,
            last_reload_error: reload.last_reload_error.clone(),
            last_reload_at_ms: reload.last_reload_at_ms,
        }
    }

    /// Reload tokens from disk. On failure, keeps the last valid set and returns Err.
    #[cfg(any(unix, test))]
    pub fn reload(&self) -> Result<(), AuthError> {
        let mut guard = self.inner.write().expect("auth lock");
        let now_ms = crate::util::time::unix_millis();
        match load_tokens_file(&guard.path) {
            Ok(loaded) => {
                guard.tokens = loaded;
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Io(String),
    Parse(String),
    DuplicateId(String),
    InvalidHash(String),
    WeakHashParams { id: String, detail: String },
    EmptyId,
    EmptyScopes { id: String },
    MissingAuth,
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
            Self::MissingAuth => {
                write!(f, "auth required: set OMAKURE_TOKENS_FILE/--tokens-file")
            }
        }
    }
}

impl std::error::Error for AuthError {}

impl AuthError {
    #[cfg(any(unix, test))]
    pub(super) fn status_message(&self) -> &'static str {
        match self {
            Self::Io(_) => "tokens file I/O error",
            Self::Parse(_) => "tokens file parse error",
            Self::DuplicateId(_) => "tokens file contains duplicate token ids",
            Self::InvalidHash(_) => "tokens file contains an invalid Argon2id hash",
            Self::WeakHashParams { .. } => "tokens file contains weak Argon2 parameters",
            Self::EmptyId => "tokens file contains an empty token id",
            Self::EmptyScopes { .. } => "tokens file contains a token with empty scopes",
            Self::MissingAuth => "authentication is not configured",
        }
    }
}
