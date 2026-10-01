use super::{
    installed_baseline_path, map_baseline_error, retained_current_path, retained_previous,
    retained_previous_path, InstalledBaseline, RetainedBaseline, BASELINE_SCRIPT_MODE,
};
use crate::baseline::VerifiedBaseline;
use crate::operations::battery::{install_verified_script, InstallState};
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::util::hex;
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

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
        match install_verified_script(workspace, Path::new(path), body, BASELINE_SCRIPT_MODE) {
            Ok(state) => staged.push(state),
            Err(error) => return Err(unwind(staged, error)),
        }
    }

    let record = InstalledBaseline {
        baseline_id: hex::encode(&baseline_id),
        publisher_key_id: hex::encode(&baseline.manifest().publisher_key_id),
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
    if let Some(error) = rotation
        .current
        .1
        .as_ref()
        .and_then(|outgoing| write_metadata_file(&previous_path, outgoing).err())
    {
        rotation.restore();
        return Err(error);
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
