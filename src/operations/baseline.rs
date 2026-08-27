//! Installing a signed baseline into a workspace: the whole set, or nothing.
//!
//! [`crate::baseline`] answers "is this exactly the set someone signed". This
//! module answers the next question — "what does it take to put that set on
//! disk without ever leaving half of it there".
//!
//! A [`VerifiedBaseline`] can only be built by `bind`, which is not
//! incremental, so by the time anything here runs every script's bytes have
//! already been checked against the signed manifest. What is left is the
//! filesystem, where all-or-nothing is not free: `N` scripts are `N` renames,
//! and the fourth can fail. Every write is therefore staged and undoable, and
//! one failure walks the successful ones back before returning.
//!
//! **Provenance is recorded for the set, not per script.** The Battery record
//! was considered and rejected on two counts. Four of its seven fields — the
//! git URL, the requested ref, the resolved commit, the source path — have no
//! meaning for a baseline and could only be filled with something untrue. More
//! seriously, `battery::installing_battery` scans that directory to answer gate
//! E of the Remote Cue plane: a baseline script recorded there would read as
//! installed *by a battery*, and any node whose `trust.remote_cue_batteries`
//! named that battery would silently have made it remotely runnable. Sharing
//! the file would have widened an authorization decision as a side effect of
//! reusing a struct.

use crate::baseline::{BaselineError, VerifiedBaseline};
use crate::operations::battery::{install_verified_script, InstallState};
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a node records about the baseline it currently holds.
///
/// One record for the set, because the set is what was signed. A per-script
/// file could go half-missing and leave the node reporting a baseline it does
/// not have, which is the drift answer wave 2 depends on being checkable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledBaseline {
    /// The derived name of the set, recomputable from the scripts on disk.
    pub baseline_id: String,
    pub publisher_key_id: String,
    pub organization: String,
    pub entries: Vec<String>,
    pub installed_at: i64,
}

/// Where the record lives, beside the Battery metadata rather than inside it.
pub fn installed_baseline_path(workspace: &Workspace) -> PathBuf {
    workspace.omakure_dir().join("baseline.json")
}

/// The baseline this node currently records, if any.
pub fn installed_baseline(workspace: &Workspace) -> Option<InstalledBaseline> {
    let contents = std::fs::read_to_string(installed_baseline_path(workspace)).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Recompute the identity of the set this node is actually holding.
///
/// This is the evidence half of drift, and it is deliberately a *recomputation*
/// rather than a re-read of the record. A node that echoed the identity it
/// wrote at install time would report exactly the same answer after every
/// script in the set had been edited underneath it, which is the one case drift
/// exists to catch.
///
/// The recorded entry list says which paths to look at, and nothing else is
/// consulted: an unlisted file in the workspace is not part of the set that was
/// published and does not change its name. A path that cannot be read — deleted,
/// replaced by a directory, or escaped from the scripts root — drops out of the
/// list, which shortens it and therefore changes the identity, which is the
/// honest answer. The empty case is safe rather than lucky: an empty entry list
/// is not signable, so the identity of "nothing readable" can never equal the
/// identity of anything that was ever pushed.
pub fn observed_baseline_id(workspace: &Workspace, record: &InstalledBaseline) -> String {
    let Ok(scripts_root) = workspace.scripts_root().canonicalize() else {
        return String::new();
    };
    let mut entries = Vec::with_capacity(record.entries.len());
    for path in &record.entries {
        let Ok(resolved) =
            crate::operations::battery::confined_existing_path(&scripts_root, Path::new(path))
        else {
            continue;
        };
        let Ok(body) = std::fs::read(&resolved) else {
            continue;
        };
        entries.push(crate::baseline::BaselineEntry {
            path: path.clone(),
            content_hash: crate::baseline::hash_script(&body),
        });
    }
    // The canonical bytes are order-sensitive and a manifest's entries are
    // sorted by path, so the list is sorted here rather than assumed: the
    // record is a file on disk and an operator can reorder it.
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    crate::baseline::derive_baseline_id(&entries)
        .map(|id| hex(&id))
        .unwrap_or_default()
}

/// One installed baseline, kept whole so this node can put itself back.
///
/// The stored payload is a `baseline_push` verbatim — the signed manifest and
/// the script bodies in manifest order — because that is what
/// [`crate::baseline_push::verify_push`] takes, and a rollback that re-asks
/// every question the push asked has to hand that function the same thing.
/// Storing a decoded form would be a second wire format for the one artefact
/// that carries code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedBaseline {
    /// The instant this node accepted the baseline, used to answer the
    /// manifest's validity window at rollback time. See
    /// [`rollback_baseline`] for why that is the one question answered as of
    /// then rather than as of now.
    pub installed_at: i64,
    pub push: serde_json::Value,
}

/// The baseline this node is running, kept whole.
pub fn retained_current_path(workspace: &Workspace) -> PathBuf {
    workspace.omakure_dir().join("baseline-current.json")
}

/// The one baseline before it. There is no third.
///
/// Exactly one is retained, and "rollback" is a swap rather than a step down a
/// stack: rolling back twice returns a machine to where it started. That is the
/// honest shape for one retained version, and it is the whole of the history
/// this plane keeps. Deeper history would need an operator vocabulary for
/// *which* version — a name, an index, a listing — that item 8 does not ask for
/// and that would grow with every push.
///
/// The cost is bounded by a bound that already exists: every installed baseline
/// arrived through a push, so its scripts are at most
/// `baseline_push::MAX_PUSH_SCRIPT_BYTES` and its manifest at most
/// `baseline::MAX_MANIFEST_BYTES`. Two slots, hexed, is under 1.3 MiB.
pub fn retained_previous_path(workspace: &Workspace) -> PathBuf {
    workspace.omakure_dir().join("baseline-previous.json")
}

/// The baseline this node would roll back to, if any.
pub fn retained_previous(workspace: &Workspace) -> Option<RetainedBaseline> {
    let contents = std::fs::read_to_string(retained_previous_path(workspace)).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Put this node back on the baseline before the one it is running.
///
/// **As verified as the push that installed it, by being the same call.** The
/// retained payload goes back through
/// [`crate::baseline_push::verify_push`] against the policy this node holds
/// *now*: the publisher must still be one it names, must not have been revoked
/// since, the organization must still match, the signature must still verify,
/// and every retained script body must still match its recorded hash. A
/// rollback to an unsigned or no-longer-verifiable state would launder code
/// past the publisher check, which is the one check this plane exists for.
///
/// **One question is answered as of then, not now: the validity window.** A
/// manifest's window bounds how long a *published artefact may be delivered* —
/// it stops a captured push being replayed onto a machine months later. Nothing
/// is delivered here; the bytes never leave the disk they are already on, and
/// this node already accepted them once, inside that window. Re-asking it as of
/// today would make rollback useless in exactly the situation it exists for: a
/// bad push discovered after the previous manifest's lifetime ran out, with the
/// publisher offline. So the window is evaluated at the instant this node
/// accepted the baseline, and every question about *whether the author is still
/// trusted* is evaluated today. The recorded instant is clamped to now, so a
/// retained record cannot reach forward into a window that has not opened.
///
/// The consequence is written down rather than hidden: there is no way for a
/// publisher to retire one specific baseline. Expiry is time-based and
/// revocation is key-wide, and neither expresses "not that one". A publisher
/// that needs a version gone revokes the key and re-signs under a new one.
///
/// Rolling back is a swap. The baseline being rolled away from becomes the
/// retained previous, so a mistaken rollback can itself be undone, and a second
/// rollback returns the machine to where it started rather than reaching for a
/// version this node never kept.
pub fn rollback_baseline(
    workspace: &Workspace,
    policy: &crate::baseline_push::BaselinePolicy,
    confirmed: bool,
    now: i64,
) -> OperationResult<InstalledBaseline> {
    // Asked for here rather than in each adapter, so the CLI and the HTTP route
    // cannot disagree about whether replacing every script a baseline named is
    // something an operator has to say out loud.
    if !confirmed {
        return Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "explicit confirmation is required to replace every script the current baseline named",
        ));
    }
    let retained = retained_previous(workspace).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::NotFound,
            "this node has no previous baseline to roll back to; exactly one is retained, \
             and nothing has replaced the one it is running",
        )
    })?;
    let push =
        crate::baseline_push::BaselinePush::parse(&retained.push).map_err(map_baseline_code)?;
    let accepted_at = retained.installed_at.min(now).max(0) as u64;
    let baseline =
        crate::baseline_push::verify_push(&push, policy, accepted_at).map_err(map_baseline_code)?;
    install_baseline(workspace, &baseline, now)
}

/// Carry the stable baseline vocabulary out of a local refusal.
///
/// The `baseline_*` names are already the frozen way this plane says why it
/// refused, and a rollback refuses for the same reasons a push does. Inventing
/// a second set of names for the local path would leave an operator comparing
/// two vocabularies for one decision.
fn map_baseline_code(code: crate::baseline_push::BaselineCode) -> OperationError {
    use crate::baseline_push::BaselineCode;
    let operation_code = match code {
        BaselineCode::TooLarge => OperationErrorCode::PayloadTooLarge,
        BaselineCode::ContentMismatch => OperationErrorCode::Conflict,
        BaselineCode::PublisherUnknown
        | BaselineCode::PublisherRevoked
        | BaselineCode::OrganizationMismatch
        | BaselineCode::SignatureMismatch
        | BaselineCode::Expired => OperationErrorCode::Forbidden,
        _ => OperationErrorCode::InvalidInput,
    };
    OperationError::new(
        operation_code,
        format!(
            "the retained baseline no longer verifies on this node: {}",
            code.name()
        ),
    )
}

/// Install every script in a verified baseline, or leave the workspace exactly
/// as it was.
///
/// The rollback is walked in reverse for no deep reason beyond symmetry with
/// the order the writes happened; each undo is independent.
pub fn install_baseline(
    workspace: &Workspace,
    baseline: &VerifiedBaseline,
    now: i64,
) -> OperationResult<InstalledBaseline> {
    let baseline_id = baseline.baseline_id().map_err(map_baseline_error)?;
    let mut staged: Vec<InstallState> = Vec::with_capacity(baseline.scripts().len());

    for (path, body) in baseline.scripts() {
        match install_verified_script(workspace, Path::new(path), body) {
            Ok(state) => staged.push(state),
            Err(error) => return Err(unwind(staged, error)),
        }
    }

    let record = InstalledBaseline {
        baseline_id: hex(&baseline_id),
        publisher_key_id: hex(&baseline.manifest().publisher_key_id),
        organization: baseline.manifest().organization.clone(),
        entries: baseline
            .scripts()
            .iter()
            .map(|(path, _)| path.clone())
            .collect(),
        installed_at: now,
    };
    let serialized = match serde_json::to_vec_pretty(&record) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Err(unwind(
                staged,
                OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to serialize the baseline record: {error}"),
                ),
            ))
        }
    };

    // The set being replaced becomes the one this node can roll back to, and
    // the set arriving becomes the one it is running. Rotating before the
    // record is written and restoring both slots on failure keeps the three
    // files describing one machine: an archive that had moved on while the
    // install was walked back would offer a rollback to the baseline the node
    // is already running.
    let rotation = match rotate_retained(workspace, baseline, now) {
        Ok(rotation) => rotation,
        Err(error) => return Err(unwind(staged, error)),
    };

    // Written last and unwound on failure, so "the scripts are installed" and
    // "the node says it holds this baseline" cannot disagree. A node that
    // reported a baseline whose scripts were not there would make wave 2's
    // drift comparison answer from a file instead of from the disk.
    if let Err(error) = write_record(workspace, &serialized) {
        rotation.restore();
        return Err(unwind(staged, error));
    }

    for mut state in staged {
        state.cleanup();
    }
    Ok(record)
}

/// The contents of both retained slots before an install touched them.
struct RetainedRotation {
    current: (PathBuf, Option<Vec<u8>>),
    previous: (PathBuf, Option<Vec<u8>>),
}

impl RetainedRotation {
    /// Put both slots back exactly as they were.
    ///
    /// Best-effort by necessity — this runs on a filesystem that has already
    /// refused something — and safe to be so, because a stale archive is not a
    /// way to install unverified code: a rollback re-verifies whatever it finds
    /// there against the publisher policy of the day.
    fn restore(self) {
        for (path, previous) in [self.previous, self.current] {
            match previous {
                Some(contents) => {
                    let _ = write_metadata_file(&path, &contents);
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}

/// Move the retained current into the previous slot and retain the new set.
fn rotate_retained(
    workspace: &Workspace,
    baseline: &VerifiedBaseline,
    now: i64,
) -> OperationResult<RetainedRotation> {
    let current_path = retained_current_path(workspace);
    let previous_path = retained_previous_path(workspace);
    let rotation = RetainedRotation {
        current: (current_path.clone(), std::fs::read(&current_path).ok()),
        previous: (previous_path.clone(), std::fs::read(&previous_path).ok()),
    };

    let bodies: Vec<Vec<u8>> = baseline
        .scripts()
        .iter()
        .map(|(_, body)| body.clone())
        .collect();
    let retained = RetainedBaseline {
        installed_at: now,
        push: crate::baseline_push::BaselinePush::encode(&baseline.manifest().encode(), &bodies),
    };
    let serialized = serde_json::to_vec(&retained).map_err(|error| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to serialize the retained baseline: {error}"),
        )
    })?;

    // A node installing its first baseline has nothing to roll back to, and
    // saying so by leaving the slot empty is what makes `rollback` refuse
    // rather than reinstall what is already there.
    if let Some(outgoing) = rotation.current.1.as_ref() {
        if let Err(error) = write_metadata_file(&previous_path, outgoing) {
            rotation.restore();
            return Err(error);
        }
    }
    if let Err(error) = write_metadata_file(&current_path, &serialized) {
        rotation.restore();
        return Err(error);
    }
    Ok(rotation)
}

/// Undo every write made so far and return the failure that caused it.
fn unwind(staged: Vec<InstallState>, error: OperationError) -> OperationError {
    for mut state in staged.into_iter().rev() {
        state.rollback();
    }
    error
}

fn write_record(workspace: &Workspace, contents: &[u8]) -> OperationResult<()> {
    write_metadata_file(&installed_baseline_path(workspace), contents)
}

/// Replace one workspace metadata file, or leave it exactly as it was.
fn write_metadata_file(path: &Path, contents: &[u8]) -> OperationResult<()> {
    let parent = path.parent().ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            "the baseline record has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to create the baseline metadata directory: {err}"),
        )
    })?;
    // A symlink here would redirect the record outside the workspace, which is
    // the same refusal the Battery metadata directory makes for the same reason.
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            "the baseline record path is a symlink",
        ));
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to stage the baseline record: {err}"),
        )
    })?;
    std::fs::rename(&temporary, path).map_err(|err| {
        let _ = std::fs::remove_file(&temporary);
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to commit the baseline record: {err}"),
        )
    })
}

fn map_baseline_error(error: BaselineError) -> OperationError {
    OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What `node baseline publish` reports about the artefact it just signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedBaseline {
    pub baseline_id: String,
    pub publisher_key_id: String,
    pub organization: String,
    pub entries: Vec<String>,
    pub manifest_path: PathBuf,
    pub manifest_bytes: usize,
    pub script_bytes: usize,
}

/// Sign the named workspace scripts as one baseline.
///
/// The bodies are read here and handed to the signer, which computes the hashes
/// itself, so this node cannot publish a manifest describing content it does
/// not hold.
///
/// The delivery bound is checked at *publish* time as well as at push time.
/// Signing something that can never be delivered would be a manifest an
/// operator has to discover is useless by trying to send it, and the answer is
/// already knowable here.
pub fn publish_baseline(
    workspace: &Workspace,
    publisher: &crate::baseline_publisher::BaselinePublisher,
    organization: &str,
    relative_paths: &[String],
    issued_at: u64,
    lifetime_seconds: u64,
    manifest_path: &Path,
) -> OperationResult<PublishedBaseline> {
    if relative_paths.is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "a baseline must name at least one script; installing nothing is not publishable",
        ));
    }
    let scripts_root = workspace.scripts_root().canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    let mut bodies = Vec::with_capacity(relative_paths.len());
    let mut script_bytes = 0usize;
    for relative in relative_paths {
        let path =
            crate::operations::battery::confined_existing_path(&scripts_root, Path::new(relative))?;
        let body = std::fs::read(&path).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to read {relative}: {err}"),
            )
        })?;
        script_bytes = script_bytes.saturating_add(body.len());
        bodies.push((relative.clone(), body));
    }
    if script_bytes > crate::baseline_push::MAX_PUSH_SCRIPT_BYTES {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!(
                "these scripts total {script_bytes} bytes, over the {} a single push may carry;                  signing it would produce a baseline that cannot be delivered",
                crate::baseline_push::MAX_PUSH_SCRIPT_BYTES
            ),
        ));
    }

    let encoded = publisher
        .publish(
            organization.to_string(),
            &bodies,
            issued_at,
            issued_at.saturating_add(lifetime_seconds),
        )
        .map_err(|error| {
            OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
        })?;
    let manifest =
        crate::baseline::SignedBaselineManifest::decode(&encoded).map_err(map_baseline_error)?;
    let baseline_id = manifest.baseline_id().map_err(map_baseline_error)?;

    std::fs::write(manifest_path, &encoded).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to write the manifest: {err}"),
        )
    })?;

    Ok(PublishedBaseline {
        baseline_id: hex(&baseline_id),
        publisher_key_id: hex(&manifest.publisher_key_id),
        organization: manifest.organization.clone(),
        entries: manifest
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect(),
        manifest_path: manifest_path.to_path_buf(),
        manifest_bytes: encoded.len(),
        script_bytes,
    })
}

/// Read the script bodies a signed manifest names, in manifest order.
///
/// Used by the push path to assemble what goes on the wire. Order comes from
/// the manifest so the array the receiver zips against its own entries can only
/// be in the order the signature covers.
pub fn bodies_for_manifest(
    workspace: &Workspace,
    manifest: &crate::baseline::SignedBaselineManifest,
) -> OperationResult<Vec<Vec<u8>>> {
    let scripts_root = workspace.scripts_root().canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    let mut bodies = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let path = crate::operations::battery::confined_existing_path(
            &scripts_root,
            Path::new(&entry.path),
        )?;
        let body = std::fs::read(&path).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to read {}: {err}", entry.path),
            )
        })?;
        // Refused here rather than left to the receiver, so an operator whose
        // workspace drifted since publishing learns it before sending rather
        // than reading a content mismatch back from every Performer.
        if crate::baseline::hash_script(&body) != entry.content_hash {
            return Err(OperationError::new(
                OperationErrorCode::Conflict,
                format!(
                    "{} no longer matches the hash this manifest recorded; publish again",
                    entry.path
                ),
            ));
        }
        bodies.push(body);
    }
    Ok(bodies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::SignedBaselineManifest;
    use crate::baseline::FUTURE_SKEW_SECONDS;
    use k256::schnorr::SigningKey;
    use sha2::{Digest, Sha256};

    const ISSUED_AT: u64 = 1_800_000_000;
    const EXPIRES_AT: u64 = 1_800_003_600;

    fn workspace(dir: &tempfile::TempDir) -> Workspace {
        let workspace = Workspace::new(dir.path().to_path_buf());
        workspace.ensure_layout().expect("layout");
        workspace
    }

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

    fn set() -> Vec<(String, Vec<u8>)> {
        vec![
            ("ops/deploy.sh".to_string(), b"echo deploy\n".to_vec()),
            ("audit.py".to_string(), b"print('audit')\n".to_vec()),
        ]
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
        let workspace = workspace(&dir);
        let baseline = verified(&set());

        let record = install_baseline(&workspace, &baseline, 1_800_000_100).expect("install");

        for (path, body) in baseline.scripts() {
            assert_eq!(
                std::fs::read(workspace.scripts_root().join(path)).expect("read installed"),
                *body,
                "{path} must be on disk with the published bytes"
            );
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
        let workspace = workspace(&dir);
        let record =
            install_baseline(&workspace, &verified(&set()), 1_800_000_100).expect("install");

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
        let workspace = workspace(&dir);
        let record =
            install_baseline(&workspace, &verified(&set()), 1_800_000_100).expect("install");

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
        let workspace = workspace(&dir);
        let record =
            install_baseline(&workspace, &verified(&set()), 1_800_000_100).expect("install");
        for (path, _) in set() {
            std::fs::remove_file(workspace.scripts_root().join(&path)).expect("remove");
        }

        let observed = observed_baseline_id(&workspace, &record);
        assert_ne!(
            observed, record.baseline_id,
            "a node holding none of its set is not in sync with it"
        );
        assert_eq!(
            observed,
            hex(&crate::baseline::derive_baseline_id(&[]).expect("the empty set has a name")),
            "the answer is the name of the empty set, which no signable baseline can equal"
        );
    }

    /// A baseline replaces what is there; that is the point of it.
    #[test]
    fn installing_over_an_existing_script_replaces_its_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = workspace(&dir);
        std::fs::create_dir_all(workspace.scripts_root().join("ops")).expect("mkdir");
        std::fs::write(
            workspace.scripts_root().join("ops/deploy.sh"),
            b"echo the old one\n",
        )
        .expect("seed");

        install_baseline(&workspace, &verified(&set()), 1_800_000_100).expect("install");

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
        let workspace = workspace(&dir);
        std::fs::write(
            workspace.scripts_root().join("audit.py"),
            b"print('the old one')\n",
        )
        .expect("seed");
        std::fs::create_dir_all(workspace.scripts_root().join("ops/deploy.sh"))
            .expect("obstruct the second script");

        install_baseline(&workspace, &verified(&set()), 1_800_000_100)
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
        let workspace = workspace(&dir);
        let first = install_baseline(&workspace, &verified(&set()), 1_800_000_100).expect("first");
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
        for (path, body) in set() {
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
        let workspace = workspace(&dir);
        install_baseline(&workspace, &verified(&set()), 1_800_000_100).expect("first");
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
        let workspace = workspace(&dir);
        install_baseline(&workspace, &verified(&set()), 1_800_000_100).expect("first");
        install_baseline(&workspace, &verified(&next_set()), 1_800_000_200).expect("second");

        let path = retained_previous_path(&workspace);
        let mut retained: RetainedBaseline =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("parse");
        // A valid script body, hexed exactly as the retained format expects,
        // that the signed manifest simply does not name the hash of.
        retained.push["scripts"][0] = serde_json::json!(hex(b"echo something else\n"));
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
        let workspace = workspace(&dir);
        install_baseline(&workspace, &verified(&set()), ISSUED_AT as i64 + 100).expect("first");
        install_baseline(&workspace, &verified(&next_set()), ISSUED_AT as i64 + 200)
            .expect("second");

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
        let workspace = workspace(&dir);

        assert!(
            install_verified_script(&workspace, Path::new("../escaped.sh"), b"echo escaped\n")
                .is_err(),
            "an install must not write outside the scripts root"
        );
        assert!(
            install_verified_script(&workspace, Path::new(".omakure/x.sh"), b"echo meta\n")
                .is_err(),
            "an install must not write into workspace metadata"
        );
        assert!(!dir.path().parent().unwrap().join("escaped.sh").exists());
    }
}
