use super::*;
use crate::test_support::baseline_scripts;
use k256::schnorr::SigningKey;
use sha2::{Digest, Sha256};

/// Baseline delivery, end to end, against the acceptance criteria for item 8.
///
/// Every test here asserts against the **workspace**, not against the reply. A
/// Performer that refuses a baseline is supposed to say very little -- for three
/// of the refusal codes, nothing at all -- so "did it install" can only be
/// answered by looking at the files. A test that read the ack would pass just
/// as well against a node that replied correctly and installed anyway.
///
/// The `BaselineSession` is driven directly rather than through two live node
/// services. The transport underneath is the same code the Cue plane already
/// certified on real sockets, and those tests are `#[ignore]`d because they
/// spawn two processes. What is new here is the gate and the install, and
/// driving the session directly is what makes the *file system* observable at
/// the moment of the decision.
#[cfg(unix)]
mod delivery;

const ISSUED_AT: u64 = 1_800_000_000;
const EXPIRES_AT: u64 = 1_800_003_600;

fn publisher(scalar: u8) -> (SigningKey, BaselinePublisherKey) {
    let signing_key = SigningKey::from_slice(&[scalar; 32]).expect("scalar");
    let mut public_key = [0u8; PUBLISHER_KEY_BYTES];
    public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
    let mut key_id = [0u8; PUBLISHER_ID_BYTES];
    key_id.copy_from_slice(&Sha256::digest(public_key)[..PUBLISHER_ID_BYTES]);
    (
        signing_key,
        BaselinePublisherKey {
            key_id,
            public_key,
            revoked: false,
        },
    )
}

fn signed(scalar: u8, bodies: &[(String, Vec<u8>)]) -> SignedBaselineManifest {
    let (signing_key, key) = publisher(scalar);
    SignedBaselineManifest::sign_with_material(
        signing_key.to_bytes().as_ref(),
        key.key_id,
        "acme".to_string(),
        bodies,
        ISSUED_AT,
        EXPIRES_AT,
    )
    .expect("sign")
}

/// Build the payload the way a sender does: bodies in manifest order.
fn push_for(manifest: &SignedBaselineManifest, bodies: &[(String, Vec<u8>)]) -> BaselinePush {
    let ordered = manifest
        .entries
        .iter()
        .map(|entry| {
            bodies
                .iter()
                .find(|(path, _)| path == &entry.path)
                .map(|(_, body)| body.clone())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    BaselinePush::parse(&BaselinePush::encode(&manifest.encode(), &ordered)).expect("parse")
}

fn policy(scalar: u8) -> BaselinePolicy {
    BaselinePolicy {
        enabled: true,
        publishers: vec![publisher(scalar).1],
        organization: "acme".to_string(),
    }
}

/// The claim the module header makes, checked against the real constants
/// and the real signer rather than against arithmetic in a comment.
///
/// This is the test that would have caught the design being wrong: a
/// maximal *signable* baseline is 256 MiB, which does not fit in a frozen
/// 1 MiB frame by any margin, and the answer was a smaller delivery bound
/// rather than a larger transport one.
#[test]
fn a_maximal_push_fits_inside_the_frozen_plaintext_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let identity = test_identity(&dir);
    let bodies: Vec<(String, Vec<u8>)> = (0..8)
        .map(|index| {
            (
                format!("bulk{index}.sh"),
                vec![b'x'; MAX_PUSH_SCRIPT_BYTES / 8],
            )
        })
        .collect();
    let manifest = signed(3, &bodies);
    let ordered: Vec<Vec<u8>> = manifest
        .entries
        .iter()
        .map(|entry| {
            bodies
                .iter()
                .find(|(path, _)| path == &entry.path)
                .expect("entry")
                .1
                .clone()
        })
        .collect();

    let envelope = crate::direct_transport::sign_baseline_envelope(
        &identity,
        KIND_PUSH,
        &[7u8; 32],
        [9u8; 16],
        BaselinePush::encode(&manifest.encode(), &ordered),
        ISSUED_AT,
    )
    .expect("sign the largest push delivery allows");

    assert!(
        envelope.encoded().len() <= crate::direct_transport::MAX_PLAINTEXT_BYTES,
        "a push at the delivery bound must fit one frame; it was {} of {}",
        envelope.encoded().len(),
        crate::direct_transport::MAX_PLAINTEXT_BYTES
    );
}

/// One byte over the bound is refused before anything is allocated for it.
#[test]
fn a_push_over_the_delivery_bound_is_refused() {
    let oversized = serde_json::json!({
        "version": 1,
        "manifest": "00",
        "scripts": [hex::encode(&vec![b'x'; MAX_PUSH_SCRIPT_BYTES + 1])],
    });
    assert_eq!(BaselinePush::parse(&oversized), Err(BaselineCode::TooLarge));
}

/// The two independent authorities: a trusted sender cannot supply an
/// untrusted publisher's baseline, and vice versa.
#[test]
fn a_baseline_from_a_publisher_this_node_does_not_name_is_refused() {
    let bodies = baseline_scripts();
    let manifest = signed(3, &bodies);
    let push = push_for(&manifest, &bodies);

    verify_push(&push, &policy(3), ISSUED_AT + 1)
        .expect("the publisher this node names must be accepted");

    assert_eq!(
        verify_push(&push, &policy(9), ISSUED_AT + 1).err(),
        Some(BaselineCode::PublisherUnknown),
        "a different publisher must not be accepted in the named one's place"
    );

    let empty = BaselinePolicy {
        publishers: Vec::new(),
        ..policy(3)
    };
    assert_eq!(
        verify_push(&push, &empty, ISSUED_AT + 1).err(),
        Some(BaselineCode::PublisherUnknown),
        "naming nobody must accept nobody"
    );

    let revoked = BaselinePolicy {
        publishers: vec![BaselinePublisherKey {
            revoked: true,
            ..publisher(3).1
        }],
        ..policy(3)
    };
    assert_eq!(
        verify_push(&push, &revoked, ISSUED_AT + 1).err(),
        Some(BaselineCode::PublisherRevoked)
    );
}

/// The set is what was signed, so one wrong script voids all of it.
#[test]
fn a_script_that_does_not_match_its_recorded_hash_voids_the_whole_push() {
    let bodies = baseline_scripts();
    let manifest = signed(3, &bodies);
    let mut push = push_for(&manifest, &bodies);

    push.bodies[0].push(b'!');
    assert_eq!(
        verify_push(&push, &policy(3), ISSUED_AT + 1).err(),
        Some(BaselineCode::ContentMismatch)
    );

    let mut short = push_for(&manifest, &bodies);
    short.bodies.pop();
    assert_eq!(
        verify_push(&short, &policy(3), ISSUED_AT + 1).err(),
        Some(BaselineCode::ContentMismatch),
        "a short array must not install a shorter set than the manifest names"
    );

    let mut extra = push_for(&manifest, &bodies);
    extra.bodies.push(b"echo extra\n".to_vec());
    assert_eq!(
        verify_push(&extra, &policy(3), ISSUED_AT + 1).err(),
        Some(BaselineCode::ContentMismatch)
    );
}

/// The organization and the validity window are checked against local
/// facts, not against anything the message asserts about itself.
#[test]
fn a_baseline_for_another_organization_or_outside_its_window_is_refused() {
    let bodies = baseline_scripts();
    let push = push_for(&signed(3, &bodies), &bodies);

    assert_eq!(
        verify_push(
            &push,
            &BaselinePolicy {
                organization: "other-org".to_string(),
                ..policy(3)
            },
            ISSUED_AT + 1,
        )
        .err(),
        Some(BaselineCode::OrganizationMismatch)
    );
    assert_eq!(
        verify_push(&push, &policy(3), EXPIRES_AT).err(),
        Some(BaselineCode::Expired)
    );
}

/// Fail-closed, one gate at a time, with a passing control so a refusal
/// cannot be mistaken for the whole thing being switched off.
#[test]
fn each_sender_gate_refuses_on_its_own() {
    let held = vec![CAPABILITY_BASELINE_PUSH.to_string()];
    let authorized = HealthAuthorization {
        node_id: "omk1_test".to_string(),
        state: PeerState::Active,
        role: PeerRole::Conductor,
        capabilities: held.clone(),
    };

    evaluate_sender_gates(true, Some(&authorized)).expect("the passing control");

    assert_eq!(
        evaluate_sender_gates(false, Some(&authorized)),
        Err(BaselineCode::Disabled),
        "the shipped default installs nothing however trusted the sender"
    );
    assert_eq!(
        evaluate_sender_gates(true, None),
        Err(BaselineCode::NotActiveConductor),
        "a peer this node has never heard of must not push code to it"
    );
    assert_eq!(
        evaluate_sender_gates(
            true,
            Some(&HealthAuthorization {
                role: PeerRole::Performer,
                ..authorized.clone()
            })
        ),
        Err(BaselineCode::NotActiveConductor)
    );
    assert_eq!(
        evaluate_sender_gates(
            true,
            Some(&HealthAuthorization {
                state: PeerState::Revoked,
                ..authorized.clone()
            })
        ),
        Err(BaselineCode::NotActiveConductor),
        "revocation must stop a push, or it is advisory"
    );
    assert_eq!(
        evaluate_sender_gates(
            true,
            Some(&HealthAuthorization {
                capabilities: vec!["remote-run".to_string()],
                ..authorized
            })
        ),
        Err(BaselineCode::MissingBaselinePush),
        "ordering a run and supplying what runs are different powers"
    );
}

/// What this node thinks of the sender, and whether the feature exists
/// here at all, are never disclosed. What it thinks of the artefact is.
#[test]
fn only_refusals_about_the_sender_are_silent() {
    for silent in [
        BaselineCode::Disabled,
        BaselineCode::NotActiveConductor,
        BaselineCode::MissingBaselinePush,
    ] {
        assert!(
            !silent.is_reportable(),
            "{} must not tell an unauthorized peer anything",
            silent.name()
        );
    }
    for reportable in [
        BaselineCode::PublisherUnknown,
        BaselineCode::PublisherRevoked,
        BaselineCode::OrganizationMismatch,
        BaselineCode::Expired,
        BaselineCode::SignatureMismatch,
        BaselineCode::ContentMismatch,
        BaselineCode::TooLarge,
        BaselineCode::InvalidMessage,
        BaselineCode::InstallFailed,
        BaselineCode::Duplicate,
    ] {
        assert!(
            reportable.is_reportable(),
            "{} is about the artefact the sender chose, and withholding it \
                 leaves an authorized Conductor guessing",
            reportable.name()
        );
    }
}

/// Every code is distinct and inside its own band.
#[test]
fn the_code_band_is_disjoint_from_every_other_plane() {
    let codes = [
        BaselineCode::Disabled,
        BaselineCode::NotActiveConductor,
        BaselineCode::MissingBaselinePush,
        BaselineCode::InvalidMessage,
        BaselineCode::TooLarge,
        BaselineCode::PublisherUnknown,
        BaselineCode::PublisherRevoked,
        BaselineCode::OrganizationMismatch,
        BaselineCode::Expired,
        BaselineCode::SignatureMismatch,
        BaselineCode::ContentMismatch,
        BaselineCode::InstallFailed,
        BaselineCode::Duplicate,
    ];
    let mut numbers = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for code in codes {
        assert!(
            (1301..=1399).contains(&code.code()),
            "{} escapes the baseline band and could collide with another plane",
            code.name()
        );
        assert!(numbers.insert(code.code()), "{} reuses a code", code.name());
        assert!(names.insert(code.name()), "{} reuses a name", code.name());
    }
}

/// A node that cannot prove what it opted into has opted into nothing.
///
/// The control comes first and is load-bearing. Without it every assertion
/// below would pass just as well if `read_policy` never returned anything
/// but the default — which is exactly the state this test was in before the
/// control was added, and it hid that the reader was refusing a perfectly
/// good config over its file mode.
#[cfg(unix)]
#[test]
fn a_config_that_cannot_be_read_denies_everything() {
    let readable = policy_from(&valid_config(), Some(0o640));
    assert!(readable.enabled, "the passing control");
    assert_eq!(readable.publishers.len(), 1);
    assert_eq!(readable.organization, "acme");

    for (label, text, mode) in [
        (
            "malformed toml",
            "this is not toml = = =".to_string(),
            Some(0o640),
        ),
        (
            "a config the validator refuses",
            valid_config().replace("port = 38383", "port = 1"),
            Some(0o640),
        ),
        (
            "a config anyone on the box can read",
            valid_config(),
            Some(0o644),
        ),
        ("no config at all", String::new(), None),
    ] {
        let policy = policy_from(&text, mode);
        assert!(
            !policy.enabled && policy.publishers.is_empty(),
            "{label} must leave the gate closed and name nobody"
        );
    }
}

#[cfg(unix)]
fn valid_config() -> String {
    let mut config = crate::domain::NodeConfig::default();
    config.trust.enrollment = "manual".to_string();
    config.trust.allow_baseline_push = true;
    config.organization.id = "acme".to_string();
    config.trust.baseline_publishers = vec![crate::domain::TrustedBaselinePublisher {
        key_id: "a".repeat(32),
        public_key: "b".repeat(64),
        revoked: false,
    }];
    config.to_toml().expect("serialize")
}

/// `mode = None` writes no config at all, which is a different failure from
/// writing one that cannot be trusted.
#[cfg(unix)]
fn policy_from(text: &str, mode: Option<u32>) -> BaselinePolicy {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("node.toml");
    if let Some(mode) = mode {
        std::fs::write(&config, text).expect("write");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }
    read_policy(&crate::test_support::node_context(dir.path()))
}

fn test_identity(dir: &tempfile::TempDir) -> crate::node_identity::NodeIdentity {
    let context = crate::test_support::configured_node_context(dir.path());
    crate::node_identity::NodeIdentity::load_or_initialize(&context).expect("identity")
}
