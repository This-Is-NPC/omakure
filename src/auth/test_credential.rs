use super::{format_toml_entry, Authenticator};
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct Credential {
    id: String,
    token: String,
    hash: String,
}

fn credential() -> &'static Credential {
    static CREDENTIAL: OnceLock<Credential> = OnceLock::new();
    CREDENTIAL.get_or_init(|| {
        toml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/test_api_token.toml"
        )))
        .expect("parse test credential fixture")
    })
}

pub(crate) fn token() -> &'static str {
    &credential().token
}

/// An authenticator whose only token is the test credential with `scopes`.
pub(crate) fn authenticator(scopes: &[&str]) -> Authenticator {
    let credential = credential();
    let scopes: Vec<String> = scopes.iter().map(|scope| scope.to_string()).collect();
    let dir = tempfile::TempDir::new().expect("tokens tempdir");
    let path = dir.path().join("tokens.toml");
    std::fs::write(
        &path,
        format!(
            "version = 1\n{}",
            format_toml_entry(&credential.id, &credential.hash, &scopes)
        ),
    )
    .expect("write test tokens file");
    Authenticator::from_tokens_file(path).expect("load test tokens file")
}
