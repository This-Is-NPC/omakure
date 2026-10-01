use super::*;

fn listing(names: &[&str]) -> Vec<std::path::PathBuf> {
    names
        .iter()
        .map(|name| std::path::PathBuf::from("/ws").join(name))
        .collect()
}

#[test]
fn a_listed_script_resolves() {
    let scripts = listing(&["deploy.sh", "backup.lua"]);
    let resolved = resolve_in_listing("deploy.sh", &scripts).expect("should resolve");
    assert_eq!(resolved, std::path::Path::new("/ws/deploy.sh"));
}

/// The listing is the allow-list, so anything absent is simply unrunnable.
#[test]
fn a_script_absent_from_the_listing_is_unresolvable() {
    let scripts = listing(&["deploy.sh"]);
    assert_eq!(
        resolve_in_listing("secret.sh", &scripts),
        Err(CueCode::ScriptUnresolvable)
    );
}

/// An `.omakureignore`d script never appears in the listing, so exclusion
/// from discovery is exclusion from remote execution with no second
/// mechanism to keep in step.
#[test]
fn an_ignored_script_is_unresolvable_because_it_is_not_listed() {
    let scripts = listing(&["public.sh"]);
    assert_eq!(
        resolve_in_listing("private.sh", &scripts),
        Err(CueCode::ScriptUnresolvable)
    );
}

/// Traversal and absolute paths die on the grammar, before any comparison.
#[test]
fn traversal_and_absolute_names_never_reach_resolution() {
    let scripts = listing(&["deploy.sh"]);
    for hostile in [
        "../deploy.sh",
        "../../etc/passwd",
        "/etc/passwd",
        "sub/deploy.sh",
        "./deploy.sh",
    ] {
        assert_eq!(
            resolve_in_listing(hostile, &scripts),
            Err(CueCode::InvalidMessage),
            "{hostile:?} must fail the grammar"
        );
    }
}

/// A name matching a listed *directory* component must not resolve.
#[test]
fn only_the_final_component_is_compared() {
    let scripts = vec![std::path::PathBuf::from("/ws/tools/deploy.sh")];
    assert_eq!(
        resolve_in_listing("tools", &scripts),
        Err(CueCode::ScriptUnresolvable)
    );
    assert!(resolve_in_listing("deploy.sh", &scripts).is_ok());
}

#[test]
fn a_symlink_is_not_a_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.sh");
    std::fs::write(&target, "#!/usr/bin/env bash\n").unwrap();
    #[cfg(unix)]
    let link = dir.path().join("link.sh");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert!(is_regular_file(&target));
    #[cfg(unix)]
    assert!(
        !is_regular_file(&link),
        "a symlink must not resolve; it could redirect outside the workspace"
    );
    assert!(!is_regular_file(dir.path()), "a directory is not a script");
    assert!(!is_regular_file(&dir.path().join("absent.sh")));
}

#[test]
fn a_secret_declaring_schema_is_detected() {
    let with_secret: crate::domain::Schema = serde_json::from_value(serde_json::json!({
        "Name": "deploy",
        "Fields": [{ "Name": "token", "Type": "secret", "Required": true }]
    }))
    .unwrap();
    assert!(declares_secret_field(&with_secret));

    let without: crate::domain::Schema = serde_json::from_value(serde_json::json!({
        "Name": "deploy",
        "Fields": [{ "Name": "target", "Type": "string", "Required": false }]
    }))
    .unwrap();
    assert!(!declares_secret_field(&without));
}

#[test]
fn the_cue_id_grammar_is_the_frozen_one() {
    assert!(is_well_formed_cue_id("0123456789abcdef0123456789abcdef"));
    for bad in [
        "",
        "0123456789abcdef0123456789abcde",
        "0123456789abcdef0123456789abcdef0",
        "0123456789ABCDEF0123456789ABCDEF",
        "0123456789abcdef0123456789abcdeg",
        "0123456789AbCdEf0123456789AbCdEf",
    ] {
        assert!(!is_well_formed_cue_id(bad), "{bad:?} should be invalid");
    }
}

#[test]
fn the_script_name_grammar_is_the_frozen_one() {
    for good in ["deploy.sh", "a", "job_1.lua", "x-y.z", &"a".repeat(64)] {
        assert!(is_well_formed_script_name(good), "{good} should be valid");
    }
    for bad in [
        "",
        ".hidden",
        "-leading",
        "_leading",
        "has space",
        "sub/dir.sh",
        "../escape.sh",
        "trailing\n",
        &"a".repeat(65),
    ] {
        assert!(
            !is_well_formed_script_name(bad),
            "{bad:?} should be invalid"
        );
    }
}

/// A file swapped after gate E must not be enqueued under that decision.
///
/// Written against `content_hash` directly because the swap window is
/// inside one function call; a test that tried to race it would be a test
/// of the scheduler, not of the guard.
#[test]
fn a_swapped_script_no_longer_matches_what_the_gate_authorized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("deploy.sh");
    std::fs::write(&script, "echo original\n").expect("write");
    let authorized = content_hash(&script).expect("hash the authorized bytes");

    std::fs::write(&script, "echo swapped\n").expect("swap");
    assert_ne!(
        content_hash(&script).as_deref(),
        Some(authorized.as_str()),
        "the accept transition must be able to see the swap"
    );

    // And a file that vanished must fail the comparison, not pass it.
    std::fs::remove_file(&script).expect("remove");
    assert_eq!(
        content_hash(&script),
        None,
        "unreadable must never compare equal to authorized"
    );
}
