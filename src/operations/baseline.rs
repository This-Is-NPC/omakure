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

    // Written last and unwound on failure, so "the scripts are installed" and
    // "the node says it holds this baseline" cannot disagree. A node that
    // reported a baseline whose scripts were not there would make wave 2's
    // drift comparison answer from a file instead of from the disk.
    if let Err(error) = write_record(workspace, &serialized) {
        return Err(unwind(staged, error));
    }

    for mut state in staged {
        state.cleanup();
    }
    Ok(record)
}

/// Undo every write made so far and return the failure that caused it.
fn unwind(staged: Vec<InstallState>, error: OperationError) -> OperationError {
    for mut state in staged.into_iter().rev() {
        state.rollback();
    }
    error
}

fn write_record(workspace: &Workspace, contents: &[u8]) -> OperationResult<()> {
    let path = installed_baseline_path(workspace);
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
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
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
    std::fs::rename(&temporary, &path).map_err(|err| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::SignedBaselineManifest;
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
