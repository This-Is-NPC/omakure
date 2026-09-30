use super::dispatch::CueDispatch;
use super::*;
use crate::util::hex;

mod gates;
mod resolution;
mod session;

#[test]
fn every_code_is_unique_and_inside_the_frozen_band() {
    let codes = [
        CueCode::Disabled,
        CueCode::NotActiveConductor,
        CueCode::MissingRemoteRun,
        CueCode::MissingNotifications,
        CueCode::NotDeclared,
        CueCode::ScriptDeclaresSecrets,
        CueCode::ScriptUnresolvable,
        CueCode::Expired,
        CueCode::Duplicate,
        CueCode::RateLimited,
        CueCode::RunAlreadyInFlight,
        CueCode::InvalidMessage,
    ];
    let mut seen = std::collections::HashSet::new();
    for code in codes {
        assert!(seen.insert(code.code()), "duplicate code {}", code.code());
        assert!((1201..=1299).contains(&code.code()));
    }
    assert_eq!(seen.len(), 12);
}

/// Deterministic, so the Conductor computes the same id the Performer will
/// use, without any message carrying a correlation field.
#[test]
fn the_run_id_is_a_deterministic_function_of_the_cue_id() {
    let a = derive_run_id("0123456789abcdef0123456789abcdef");
    assert_eq!(a, derive_run_id("0123456789abcdef0123456789abcdef"));
    assert_ne!(a, derive_run_id("fedcba9876543210fedcba9876543210"));
    assert_eq!(a.len(), 64, "a full SHA-256 in hex");
}

/// The domain separator is load-bearing: without it a cue id would hash the
/// same here as in any other construction that hashes ids.
#[test]
fn the_run_id_is_domain_separated() {
    use sha2::{Digest, Sha256};
    let undomained: String = hex::encode(&Sha256::digest(b"0123456789abcdef0123456789abcdef"));
    assert_ne!(
        derive_run_id("0123456789abcdef0123456789abcdef"),
        undomained
    );
}

#[test]
fn not_declared_is_reported_as_unresolvable() {
    assert_eq!(
        CueCode::NotDeclared.reply_code(),
        CueCode::ScriptUnresolvable,
        "the difference would let an authorized peer enumerate the workspace"
    );
    assert_ne!(
        CueCode::NotDeclared.code(),
        CueCode::ScriptUnresolvable.code(),
        "but the receiving operator must still see which one it was"
    );
}

/// Every other code reports as itself; only the enumeration case collapses.
#[test]
fn no_other_code_is_disguised() {
    for code in [
        CueCode::Disabled,
        CueCode::NotActiveConductor,
        CueCode::MissingRemoteRun,
        CueCode::MissingNotifications,
        CueCode::ScriptDeclaresSecrets,
        CueCode::ScriptUnresolvable,
        CueCode::Expired,
        CueCode::Duplicate,
        CueCode::RateLimited,
        CueCode::RunAlreadyInFlight,
        CueCode::InvalidMessage,
    ] {
        assert_eq!(
            code.reply_code(),
            code,
            "{} must report as itself",
            code.name()
        );
    }
}

#[test]
fn a_cue_outside_its_window_is_expired_rather_than_accepted() {
    assert_eq!(within_validity_window(100, 400, 99), Err(CueCode::Expired));
    assert_eq!(within_validity_window(100, 400, 400), Err(CueCode::Expired));
    assert_eq!(within_validity_window(100, 400, 100), Ok(()));
    assert_eq!(within_validity_window(100, 400, 399), Ok(()));
}

#[test]
fn a_window_wider_than_the_frozen_lifetime_is_malformed() {
    assert_eq!(
        within_validity_window(100, 100 + MAX_LIFETIME_SECONDS + 1, 150),
        Err(CueCode::InvalidMessage)
    );
    assert_eq!(
        within_validity_window(100, 100, 100),
        Err(CueCode::InvalidMessage)
    );
    assert_eq!(
        within_validity_window(400, 100, 150),
        Err(CueCode::InvalidMessage)
    );
}

#[test]
fn cue_payload_rejects_unknown_fields() {
    let payload = serde_json::json!({
        "version": 1,
        "cue_id": "0123456789abcdef0123456789abcdef",
        "script": "deploy.sh",
        "not_before": 100,
        "expires_at": 200,
        "reason": "approved",
        "arguments": []
    });
    assert!(CueDispatch::parse(&payload).is_none());
}

#[test]
fn cue_retained_state_bounds_are_frozen() {
    assert_eq!(MAX_RETAINED_CUE_RECORDS, 64);
    assert_eq!(CUE_RETENTION_SECONDS, 604800);
    assert_eq!(MAX_CANONICAL_CUE_DISPATCH, 512);
}
