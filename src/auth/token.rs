use super::types::AuthError;
use crate::util::hex;
use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;
use rand::RngCore;

pub const TOKEN_PREFIX: &str = "omk_live_";
/// Recommended Argon2id parameters (64 MiB, t=3, p=1).
const ARGON2_M_COST: u32 = 65536;
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 1;
pub(super) const PLAINTEXT_BYTES: usize = 32;

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

pub(super) fn selector_token_plaintext(id: &str) -> String {
    let mut bytes = [0u8; PLAINTEXT_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let mut encoded =
        String::with_capacity(TOKEN_PREFIX.len() + id.len() * 2 + 1 + bytes.len() * 2);
    encoded.push_str(TOKEN_PREFIX);
    encoded.push_str(&hex::encode(id.as_bytes()));
    encoded.push('_');
    encoded.push_str(&hex::encode(&bytes));
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
