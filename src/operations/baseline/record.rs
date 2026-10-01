use crate::util::hex;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A baseline script arrives as verified bytes, with no file whose mode could
/// be kept. It is installed to be run, so it is installed executable.
pub const BASELINE_SCRIPT_MODE: u32 = 0o755;

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
        .map(|id| hex::encode(&id))
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
