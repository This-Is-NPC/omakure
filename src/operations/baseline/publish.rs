use super::map_baseline_error;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::util::hex;
use crate::workspace::Workspace;
use serde::Serialize;
use std::path::{Path, PathBuf};

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
        baseline_id: hex::encode(&baseline_id),
        publisher_key_id: hex::encode(&manifest.publisher_key_id),
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
