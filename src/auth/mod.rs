//! Multi-token bearer auth: `--tokens-file` / `OMAKURE_TOKENS_FILE` TOML with
//! per-token Argon2id hashes and scopes.

mod append;
mod bearer;
mod file;
mod reload;
mod scope;
mod token;
mod types;

/// The shared test credential in `tests/fixtures/test_api_token.toml`.
#[cfg(test)]
pub(crate) mod test_credential;
#[cfg(test)]
mod tests;

pub use append::append_token_entry;
#[cfg(test)]
pub use file::{load_tokens_file, parse_tokens_toml, MAX_TOKENS_PER_FILE};
pub use reload::install_sighup_reload;
pub use token::generate_token;
#[cfg(test)]
pub use token::test_token_plaintext;
#[cfg(test)]
pub use token::{format_toml_entry, hash_token, TOKEN_PREFIX};
#[cfg(test)]
pub use types::TokenRecord;
pub use types::{AuthContext, AuthError, AuthStatus, Authenticator};

use std::path::Path;

/// Resolve the authenticator from the configured tokens file.
pub fn resolve_authenticator(tokens_file: Option<&Path>) -> Result<Authenticator, AuthError> {
    Authenticator::from_tokens_file(tokens_file.ok_or(AuthError::MissingAuth)?)
}
