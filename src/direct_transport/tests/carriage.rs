use super::*;

/// The health signer must stay a health signer.
///
/// The Remote Cue plane is a sibling that reuses the private, kind-agnostic
/// `sign_envelope`. If `sign_health_envelope` ever accepted a `cue_` kind it
/// would become a generic signing oracle for a plane it does not govern, and
/// the separation the Cue contract rests on would be gone.
#[test]
fn sign_health_envelope_refuses_a_foreign_plane_kind() {
    let dir = TempDir::new().unwrap();
    let identity = identity(&dir);
    for foreign in ["cue_dispatch", "cue_ack", "baseline_push", "probe", ""] {
        let result = sign_health_envelope(
            &identity,
            foreign,
            &[7u8; 32],
            [9u8; 16],
            serde_json::json!({"version": 1}),
            1_800_000_000,
        );
        assert!(
            matches!(result, Err(TransportError::InvalidFrame)),
            "sign_health_envelope must refuse {foreign:?}"
        );
    }
}

/// And the symmetry: the Cue signer must refuse health kinds just as
/// firmly, or the separation only holds in one direction.
#[test]
fn sign_cue_envelope_refuses_a_foreign_plane_kind() {
    let dir = TempDir::new().unwrap();
    let identity = identity(&dir);
    for foreign in [
        "health_profile",
        "health_signal",
        "baseline_push",
        "probe",
        "",
    ] {
        let result = sign_cue_envelope(
            &identity,
            foreign,
            &[7u8; 32],
            [9u8; 16],
            serde_json::json!({"version": 1}),
            1_800_000_000,
        );
        assert!(
            matches!(result, Err(TransportError::InvalidFrame)),
            "sign_cue_envelope must refuse {foreign:?}"
        );
    }
}

#[test]
fn sign_cue_envelope_signs_its_own_namespace() {
    let dir = TempDir::new().unwrap();
    let identity = identity(&dir);
    for own in ["cue_dispatch", "cue_ack"] {
        sign_cue_envelope(
            &identity,
            own,
            &[7u8; 32],
            [9u8; 16],
            serde_json::json!({"version": 1}),
            1_800_000_000,
        )
        .unwrap_or_else(|_| panic!("sign_cue_envelope must sign {own}"));
    }
}

/// The third wrapper, held to the same rule in both directions.
///
/// A baseline is the only message on this transport that carries code, so
/// a signer that could be talked into minting one under another plane's
/// name is the worst version of this mistake available.
#[test]
fn sign_baseline_envelope_signs_only_its_own_namespace() {
    let dir = TempDir::new().unwrap();
    let identity = identity(&dir);
    for foreign in [
        "health_profile",
        "health_signal",
        "cue_dispatch",
        "probe",
        "",
    ] {
        assert!(
            matches!(
                sign_baseline_envelope(
                    &identity,
                    foreign,
                    &[7u8; 32],
                    [9u8; 16],
                    serde_json::json!({"version": 1}),
                    1_800_000_000,
                ),
                Err(TransportError::InvalidFrame)
            ),
            "sign_baseline_envelope must refuse {foreign:?}"
        );
    }
    for own in ["baseline_push", "baseline_ack"] {
        sign_baseline_envelope(
            &identity,
            own,
            &[7u8; 32],
            [9u8; 16],
            serde_json::json!({"version": 1}),
            1_800_000_000,
        )
        .unwrap_or_else(|_| panic!("sign_baseline_envelope must sign {own}"));
    }
}
