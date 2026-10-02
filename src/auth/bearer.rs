use super::token::{PLAINTEXT_BYTES, TOKEN_PREFIX};
use super::types::{AuthContext, TokenRecord};
use crate::util::hex;
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordVerifier};

#[cfg(test)]
thread_local! {
    pub(super) static ARGON2_VERIFY_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Bearers without a token selector cannot name a record, so they are
/// rejected before any Argon2 work (keeps the auth-flood bound effective
/// against arbitrary bearer strings).
pub(super) fn authenticate_against_file(
    tokens: &[TokenRecord],
    presented: &str,
) -> Option<AuthContext> {
    let id = token_selector(presented)?;
    tokens
        .iter()
        .find(|token| token.enabled && token.id == id)
        .filter(|token| verify_argon2(&token.hash, presented))
        .map(|token| AuthContext {
            token_id: token.id.clone(),
            scopes: token.scopes.clone(),
        })
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
    String::from_utf8(hex::decode(encoded_id)?).ok()
}

pub(super) fn verify_argon2(phc: &str, presented: &str) -> bool {
    #[cfg(test)]
    ARGON2_VERIFY_COUNT.with(|count| count.set(count.get() + 1));
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(presented.as_bytes(), &parsed)
        .is_ok()
}
