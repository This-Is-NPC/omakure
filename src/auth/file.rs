use super::types::{AuthError, TokenRecord};
use argon2::password_hash::PasswordHash;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Reject weaker hashes unless explicitly allowed for tests/dev.
const MIN_M_COST: u32 = 19_456; // ~19 MiB floor for containers
const MIN_T_COST: u32 = 2;
const MIN_P_COST: u32 = 1;

pub const MAX_TOKENS_PER_FILE: usize = 64;

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
