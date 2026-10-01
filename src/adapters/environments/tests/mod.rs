#[cfg(unix)]
use super::files::write_file_atomic;
use super::layers::{parse_env_defaults, parse_env_preview};
use super::values::value_contains_credentials;
use super::*;
use pretty_assertions::assert_eq;
use rstest::rstest;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

mod layers;
mod repository;

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
    assert!(
        old_predictable_tmp
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
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

#[rstest]
#[case::postgres_password("postgres://user:pass@localhost/db", true)]
#[case::https_basic_auth("https://user:pass@example.com/path", true)]
#[case::no_password("postgres://user@localhost/db", false)]
#[case::no_user("postgres://localhost/db", false)]
#[case::not_url("user:pass@localhost", false)]
fn test_value_contains_credentials(#[case] value: &str, #[case] expected: bool) {
    assert_eq!(value_contains_credentials(value), expected);
}
