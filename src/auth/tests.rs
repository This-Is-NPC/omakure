use super::bearer::{ARGON2_VERIFY_COUNT, authenticate_against_file, verify_argon2};
use super::token::{PLAINTEXT_BYTES, selector_token_plaintext};
use super::*;
use std::fs;
use std::path::Path;
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

    let selectorless = format!("{TOKEN_PREFIX}{}", "ab".repeat(PLAINTEXT_BYTES));
    assert!(authenticate_against_file(&tokens, &selectorless).is_none());
    ARGON2_VERIFY_COUNT.with(|count| assert_eq!(count.get(), 0));
}

#[test]
fn bearer_without_selector_does_no_argon2_work() {
    let plaintext = test_token_plaintext("configured");
    let hash = hash_token(&plaintext).unwrap();
    let tokens = (0..MAX_TOKENS_PER_FILE)
        .map(|index| TokenRecord {
            id: format!("configured-{index}"),
            hash: hash.clone(),
            scopes: vec!["runs:read".into()],
            enabled: true,
        })
        .collect::<Vec<_>>();

    // No TOKEN_PREFIX at all — must never trigger Argon2 work.
    ARGON2_VERIFY_COUNT.with(|count| count.set(0));
    assert!(authenticate_against_file(&tokens, "invalid bearer").is_none());
    ARGON2_VERIFY_COUNT.with(|count| assert_eq!(count.get(), 0));
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
fn generate_token_uses_prefix_and_verifiable_hash() {
    let gen = generate_token("ci", &["runs:read".into(), "scripts:read".into()]).unwrap();
    assert!(gen.token.starts_with(TOKEN_PREFIX));
    assert!(gen.hash.contains("argon2id"));
    assert!(gen.tokens_file_entry.contains("id = \"ci\""));
    assert!(verify_argon2(&gen.hash, &gen.token));
    assert!(!gen.tokens_file_entry.contains(&gen.token));
    let parsed = argon2::password_hash::PasswordHash::new(&gen.hash).unwrap();
    let mut salt_bytes = [0u8; argon2::password_hash::Salt::RECOMMENDED_LENGTH];
    assert_eq!(
        parsed
            .salt
            .unwrap()
            .decode_b64(&mut salt_bytes)
            .unwrap()
            .len(),
        salt_bytes.len()
    );
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
    assert!(
        after
            .last_reload_error
            .as_deref()
            .is_some_and(|e| !e.is_empty())
    );
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
    use std::process::{Command, Stdio};

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
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }

    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "append worker failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let tokens = load_tokens_file(&path).unwrap();
    assert_eq!(tokens.len(), 6);
    for index in 0..6 {
        assert!(
            tokens
                .iter()
                .any(|token| token.id == format!("process-{index}"))
        );
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
