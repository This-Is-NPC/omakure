use super::*;
use crate::baseline::SignedBaselineManifest;
use crate::baseline::VerifiedBaseline;
use crate::baseline::FUTURE_SKEW_SECONDS;
use crate::operations::battery::install_verified_script;
use crate::operations::OperationErrorCode;
use crate::test_support::{baseline_scripts, workspace_in};
use crate::util::hex;
use k256::schnorr::SigningKey;
use sha2::{Digest, Sha256};
use std::path::Path;

const ISSUED_AT: u64 = 1_800_000_000;
const EXPIRES_AT: u64 = 1_800_003_600;

fn verified(bodies: &[(String, Vec<u8>)]) -> VerifiedBaseline {
    let signing_key = SigningKey::from_slice(&[7u8; 32]).expect("scalar");
    let mut public_key = [0u8; 32];
    public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
    let mut key_id = [0u8; 16];
    key_id.copy_from_slice(&Sha256::digest(public_key)[..16]);
    SignedBaselineManifest::sign_with_material(
        signing_key.to_bytes().as_ref(),
        key_id,
        "acme".to_string(),
        bodies,
        ISSUED_AT,
        EXPIRES_AT,
    )
    .expect("sign")
    .bind(bodies.to_vec())
    .expect("bind")
}

/// The next version of the same set: same paths, different bytes.
fn next_set() -> Vec<(String, Vec<u8>)> {
    vec![
        ("ops/deploy.sh".to_string(), b"echo deploy v2\n".to_vec()),
        ("audit.py".to_string(), b"print('audit v2')\n".to_vec()),
    ]
}

/// The publisher `verified` signs with, as a receiver would record it.
fn policy(revoked: bool) -> crate::baseline_push::BaselinePolicy {
    let signing_key = SigningKey::from_slice(&[7u8; 32]).expect("scalar");
    let mut public_key = [0u8; 32];
    public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
    let mut key_id = [0u8; 16];
    key_id.copy_from_slice(&Sha256::digest(public_key)[..16]);
    crate::baseline_push::BaselinePolicy {
        enabled: true,
        publishers: vec![crate::baseline::BaselinePublisherKey {
            key_id,
            public_key,
            revoked,
        }],
        organization: "acme".to_string(),
    }
}

/// The whole set lands, and the record names the set that landed.
#[test]
fn a_verified_baseline_installs_every_script_and_records_the_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    let baseline = verified(&baseline_scripts());

    let record = install_baseline(&workspace, &baseline, 1_800_000_100).expect("install");

    for (path, body) in baseline.scripts() {
        assert_eq!(
            std::fs::read(workspace.scripts_root().join(path)).expect("read installed"),
            *body,
            "{path} must be on disk with the published bytes"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(workspace.scripts_root().join(path))
                .expect("stat installed")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode, BASELINE_SCRIPT_MODE,
                "{path} must be installed executable"
            );
        }
    }
    assert_eq!(
        record.baseline_id,
        installed_baseline(&workspace).expect("record").baseline_id,
        "the record on disk must name the baseline that was installed"
    );
    assert_eq!(record.entries.len(), 2);
}

/// Drift is a recomputation, not a re-read of what was recorded.
///
/// Every case here changes the *scripts* and asks what the node now holds,
/// because a check that only ever compares the record to itself would pass
/// on a machine whose whole set had been rewritten underneath it.
#[test]
fn the_observed_identity_follows_the_scripts_and_not_the_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    let record = install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100)
        .expect("install");

    assert_eq!(
        observed_baseline_id(&workspace, &record),
        record.baseline_id,
        "a node running what it installed must recompute the identity it recorded"
    );

    // A legitimate-looking edit: still a valid script, still at its path.
    std::fs::write(
        workspace.scripts_root().join("ops/deploy.sh"),
        b"echo deploy\necho and one more thing\n",
    )
    .expect("edit the script underneath the node");
    let edited = observed_baseline_id(&workspace, &record);
    assert_ne!(
        edited, record.baseline_id,
        "one script changed underneath the node must change what it observes"
    );
    assert!(
        !edited.is_empty(),
        "a drifted node still holds a set, and reporting nothing would read as never pushed"
    );

    std::fs::write(
        workspace.scripts_root().join("ops/deploy.sh"),
        b"echo deploy\n",
    )
    .expect("put the bytes back");
    assert_eq!(
        observed_baseline_id(&workspace, &record),
        record.baseline_id,
        "restoring the published bytes must restore the identity, or drift is one-way"
    );

    std::fs::remove_file(workspace.scripts_root().join("audit.py")).expect("delete");
    assert_ne!(
        observed_baseline_id(&workspace, &record),
        record.baseline_id,
        "a script the set names and the disk no longer has is drift"
    );
}

/// The identity names the set that was published, and says nothing about
/// what else is on the machine.
///
/// Written down as a test rather than left to the reader: an operator who
/// believed `in_sync` meant "nothing else here" would be wrong, and the
/// place to be honest about that is beside the code that decides it.
#[test]
fn a_file_no_baseline_entry_names_does_not_change_the_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    let record = install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100)
        .expect("install");

    std::fs::write(
        workspace.scripts_root().join("unlisted.sh"),
        b"echo not part of any baseline\n",
    )
    .expect("write an unlisted script");

    assert_eq!(
        observed_baseline_id(&workspace, &record),
        record.baseline_id,
        "the set that was signed is unchanged, and drift must not claim otherwise"
    );
}

/// The one identity that can never be mistaken for being in sync.
#[test]
fn a_node_that_can_read_none_of_its_set_cannot_read_as_in_sync() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    let record = install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100)
        .expect("install");
    for (path, _) in baseline_scripts() {
        std::fs::remove_file(workspace.scripts_root().join(&path)).expect("remove");
    }

    let observed = observed_baseline_id(&workspace, &record);
    assert_ne!(
        observed, record.baseline_id,
        "a node holding none of its set is not in sync with it"
    );
    assert_eq!(
        observed,
        hex::encode(&crate::baseline::derive_baseline_id(&[]).expect("the empty set has a name")),
        "the answer is the name of the empty set, which no signable baseline can equal"
    );
}

/// A baseline replaces what is there; that is the point of it.
#[test]
fn installing_over_an_existing_script_replaces_its_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    std::fs::create_dir_all(workspace.scripts_root().join("ops")).expect("mkdir");
    std::fs::write(
        workspace.scripts_root().join("ops/deploy.sh"),
        b"echo the old one\n",
    )
    .expect("seed");

    install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100).expect("install");

    assert_eq!(
        std::fs::read(workspace.scripts_root().join("ops/deploy.sh")).expect("read"),
        b"echo deploy\n".to_vec()
    );
}

/// The property that keeps a half-installed fleet off the map: one script
/// that cannot be written puts every earlier one back.
///
/// `audit.py` sorts before `ops/deploy.sh`, so it is installed first and a
/// failure on the second has something to undo. It is seeded with different
/// bytes on purpose: asserting the file is *absent* afterwards would also
/// pass if the write had never happened, which is not the same property.
/// Asserting the *old bytes survived* can only be true if the new ones were
/// written and then taken back.
///
/// The obstruction is a directory standing where a file must go — a real
/// filesystem refusal down the same path a full disk or a permission
/// denial takes, rather than an injected error.
#[test]
#[cfg(unix)]
fn one_unwritable_script_leaves_the_workspace_exactly_as_it_was() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    std::fs::write(
        workspace.scripts_root().join("audit.py"),
        b"print('the old one')\n",
    )
    .expect("seed");
    std::fs::create_dir_all(workspace.scripts_root().join("ops/deploy.sh"))
        .expect("obstruct the second script");

    install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100)
        .expect_err("a script that cannot be written must fail the whole set");

    assert_eq!(
        std::fs::read(workspace.scripts_root().join("audit.py")).expect("read"),
        b"print('the old one')\n".to_vec(),
        "the script installed before the failure must have been walked back"
    );
    assert!(
        installed_baseline(&workspace).is_none(),
        "a failed install must not record a baseline the node does not hold"
    );
}

/// Rollback restores the previous version and leaves the node in sync
/// against it.
#[test]
fn a_rollback_restores_the_previous_set_and_the_node_reads_as_in_sync() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    let first =
        install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100).expect("first");
    let second =
        install_baseline(&workspace, &verified(&next_set()), 1_800_000_200).expect("second");
    assert_ne!(first.baseline_id, second.baseline_id);
    assert_eq!(
        std::fs::read(workspace.scripts_root().join("ops/deploy.sh")).expect("read"),
        b"echo deploy v2\n".to_vec()
    );

    let restored = rollback_baseline(&workspace, &policy(false), true, 1_800_000_300)
        .expect("a set this node installed under a publisher it still names rolls back");

    assert_eq!(
        restored.baseline_id, first.baseline_id,
        "rollback restores the version before the current one"
    );
    for (path, body) in baseline_scripts() {
        assert_eq!(
            std::fs::read(workspace.scripts_root().join(&path)).expect("read"),
            body,
            "{path} must hold the bytes of the restored set"
        );
    }
    assert_eq!(
        observed_baseline_id(&workspace, &restored),
        first.baseline_id,
        "a rolled-back node reports in sync against the version it was put back on"
    );

    // Exactly one version is retained, so this is a swap and not a stack.
    let again = rollback_baseline(&workspace, &policy(false), true, 1_800_000_400)
        .expect("the set rolled away from is the one now retained");
    assert_eq!(
        again.baseline_id, second.baseline_id,
        "rolling back twice returns this node to where it started"
    );
}

/// A publisher revoked since the install must make the rollback fail.
///
/// This is the property that separates a rollback from copying files back:
/// the retained set goes through the same `verify_push` the delivery path
/// runs, against the policy this node holds *today*. Without it, a machine
/// could be walked back onto code whose author the fleet had since
/// disowned, with no signature check anywhere in the story.
#[test]
fn a_rollback_under_a_revoked_publisher_is_refused_and_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100).expect("first");
    let second =
        install_baseline(&workspace, &verified(&next_set()), 1_800_000_200).expect("second");

    let refused = rollback_baseline(&workspace, &policy(true), true, 1_800_000_300)
        .expect_err("a revoked publisher's code must not be reinstalled");
    assert_eq!(refused.code, OperationErrorCode::Forbidden);
    assert!(
        refused.message.contains("baseline_publisher_revoked"),
        "the stable baseline vocabulary must say why: {}",
        refused.message
    );

    // A named-but-different publisher, and an unnamed one: the same refusal
    // reached two other ways, each leaving the machine exactly as it was.
    let mut stranger = policy(false);
    stranger.publishers[0].public_key[0] ^= 0xff;
    assert!(rollback_baseline(&workspace, &stranger, true, 1_800_000_300).is_err());
    assert!(rollback_baseline(
        &workspace,
        &crate::baseline_push::BaselinePolicy::default(),
        true,
        1_800_000_300
    )
    .is_err());

    assert_eq!(
        installed_baseline(&workspace).expect("record").baseline_id,
        second.baseline_id,
        "a refused rollback leaves the node on the baseline it was running"
    );
    assert_eq!(
        std::fs::read(workspace.scripts_root().join("ops/deploy.sh")).expect("read"),
        b"echo deploy v2\n".to_vec(),
        "a refused rollback must not put any of the old bytes back"
    );
}

/// A retained set tampered with on disk is refused by the content check.
///
/// The signature covers every script's hash, so an operator who edited the
/// retained copy cannot use rollback as a way to install it.
#[test]
fn a_tampered_retained_set_cannot_be_rolled_back_into_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    install_baseline(&workspace, &verified(&baseline_scripts()), 1_800_000_100).expect("first");
    install_baseline(&workspace, &verified(&next_set()), 1_800_000_200).expect("second");

    let path = retained_previous_path(&workspace);
    let mut retained: RetainedBaseline =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("parse");
    // A valid script body, hexed exactly as the retained format expects,
    // that the signed manifest simply does not name the hash of.
    retained.push["scripts"][0] = serde_json::json!(hex::encode(b"echo something else\n"));
    std::fs::write(&path, serde_json::to_vec(&retained).expect("serialize")).expect("write");

    let refused = rollback_baseline(&workspace, &policy(false), true, 1_800_000_300)
        .expect_err("a retained body the manifest does not name must be refused");
    assert_eq!(refused.code, OperationErrorCode::Conflict);
    assert!(refused.message.contains("baseline_content_mismatch"));
}

/// The window is answered as of the install, and cannot reach forward.
#[test]
fn a_rollback_survives_the_manifests_expiry_but_not_a_window_that_never_opened() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);
    install_baseline(
        &workspace,
        &verified(&baseline_scripts()),
        ISSUED_AT as i64 + 100,
    )
    .expect("first");
    install_baseline(&workspace, &verified(&next_set()), ISSUED_AT as i64 + 200).expect("second");

    // Long past the manifest's own expiry. Nothing is delivered by a
    // rollback, and this node already accepted these bytes inside the
    // window, so the question is answered as of then.
    let restored = rollback_baseline(
        &workspace,
        &policy(false),
        true,
        EXPIRES_AT as i64 + 90 * 24 * 60 * 60,
    )
    .expect("an expired manifest still names a set this node already ran");
    assert_eq!(restored.entries.len(), 2);

    // A retained record claiming to have been installed before its own
    // manifest was issued must not reach a window that had not opened.
    let path = retained_previous_path(&workspace);
    let mut retained: RetainedBaseline =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("parse");
    retained.installed_at = ISSUED_AT as i64 - FUTURE_SKEW_SECONDS as i64 - 10;
    std::fs::write(&path, serde_json::to_vec(&retained).expect("serialize")).expect("write");
    let refused = rollback_baseline(&workspace, &policy(false), true, ISSUED_AT as i64 + 400)
        .expect_err("a window that had not opened is still a refusal");
    assert!(refused.message.contains("baseline_expired"));
}

/// A path the manifest could never carry, asked for directly.
///
/// `validate_entry_path` refuses traversal at signing time, so this can only
/// be reached by a caller that built a `VerifiedBaseline` some other way --
/// which is exactly why the install refuses it again rather than trusting
/// that the earlier check ran.
#[test]
fn the_install_confines_paths_itself_rather_than_trusting_the_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = workspace_in(&dir);

    assert!(
        install_verified_script(
            &workspace,
            Path::new("../escaped.sh"),
            b"echo escaped\n",
            BASELINE_SCRIPT_MODE,
        )
        .is_err(),
        "an install must not write outside the scripts root"
    );
    assert!(
        install_verified_script(
            &workspace,
            Path::new(".omakure/x.sh"),
            b"echo meta\n",
            BASELINE_SCRIPT_MODE,
        )
        .is_err(),
        "an install must not write into workspace metadata"
    );
    assert!(!dir.path().parent().unwrap().join("escaped.sh").exists());
}
