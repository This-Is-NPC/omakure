use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::files::open_existing_file_no_follow;
use super::install::verify_tracked_blob;
use super::path_safety::{
    confined_existing_path, reject_reserved_install_path, reject_symlink_components,
    reject_unsafe_relative_path,
};
use super::registry::cache_path_for_battery;
use crate::domain::{extract_schema_block, parse_schema};
use crate::runtime::{ScriptKind, script_kind};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "omakure-battery.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifest {
    pub battery: BatteryManifestHeader,
    #[serde(default)]
    pub scripts: Vec<BatteryManifestScript>,
    #[serde(default)]
    pub workflows: Vec<BatteryManifestWorkflow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifestHeader {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifestScript {
    pub id: String,
    pub path: PathBuf,
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifestWorkflow {
    pub id: String,
    pub description: Option<String>,
    pub scripts: Vec<String>,
}

pub fn parse_manifest(contents: &str) -> OperationResult<BatteryManifest> {
    toml::from_str(contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("battery manifest is invalid: {err}"),
        )
    })
}

pub fn load_manifest(cache_path: &Path) -> OperationResult<BatteryManifest> {
    let manifest_rel = Path::new(MANIFEST_FILE);
    reject_symlink_components(cache_path, manifest_rel, true)?;
    let manifest_path = confined_existing_path(cache_path, manifest_rel)?;
    let contents = fs::read_to_string(&manifest_path).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("failed to read battery manifest: {err}"),
        )
    })?;
    parse_manifest(&contents)
}

pub fn validate_manifest(cache_path: &Path, manifest: &BatteryManifest) -> OperationResult<()> {
    if manifest.battery.name.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            "battery manifest name is required",
        ));
    }
    let mut ids = HashSet::new();
    for script in &manifest.scripts {
        if !ids.insert(script.id.as_str()) {
            return Err(OperationError::new(
                OperationErrorCode::ManifestInvalid,
                format!("duplicate battery script id: {}", script.id),
            ));
        }
        validate_script_entry(cache_path, script)?;
    }
    let mut workflow_ids = HashSet::new();
    for workflow in &manifest.workflows {
        if workflow.id.trim().is_empty() {
            return Err(OperationError::new(
                OperationErrorCode::ManifestInvalid,
                "battery workflow id is required",
            ));
        }
        if !workflow_ids.insert(workflow.id.as_str()) {
            return Err(OperationError::new(
                OperationErrorCode::ManifestInvalid,
                format!("duplicate battery workflow id: {}", workflow.id),
            ));
        }
        if workflow.scripts.len() < 2 {
            return Err(OperationError::new(
                OperationErrorCode::ManifestInvalid,
                format!(
                    "battery workflow '{}' requires at least two steps",
                    workflow.id
                ),
            ));
        }
        for script_id in &workflow.scripts {
            if !ids.contains(script_id.as_str()) {
                return Err(OperationError::new(
                    OperationErrorCode::ManifestInvalid,
                    format!(
                        "battery workflow '{}' references unknown script id: {}",
                        workflow.id, script_id
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_manifest_for_battery(
    cache_path: &Path,
    manifest: &BatteryManifest,
    battery_name: &str,
) -> OperationResult<()> {
    validate_manifest(cache_path, manifest)?;
    if manifest.battery.name != battery_name {
        return Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!(
                "battery manifest name '{}' does not match registration '{}'",
                manifest.battery.name, battery_name
            ),
        ));
    }
    Ok(())
}

pub fn validate_script_entry(
    cache_path: &Path,
    script: &BatteryManifestScript,
) -> OperationResult<PathBuf> {
    open_validated_script_entry(cache_path, script).map(|(path, _)| path)
}

pub fn installed_script_matches_manifest(
    workspace: &Workspace,
    battery_name: &str,
    script: &BatteryManifestScript,
) -> OperationResult<Option<String>> {
    let cache_path = cache_path_for_battery(workspace, battery_name)?;
    let (_, mut source) = open_validated_script_entry(&cache_path, script)?;
    let scripts_root = workspace.scripts_root();
    reject_symlink_components(scripts_root, &script.path, true)?;
    let mut installed = open_existing_file_no_follow(&scripts_root.join(&script.path))?;
    let mut source_buffer = [0_u8; 8192];
    let mut installed_buffer = [0_u8; 8192];
    let mut source_hash = Sha256::new();
    loop {
        let count = source.read(&mut source_buffer).map_err(|err| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("failed to read Battery source script: {err}"),
            )
        })?;
        if count == 0 {
            return installed
                .read(&mut installed_buffer[..1])
                .map(|n| (n == 0).then(|| crate::util::hex::encode(&source_hash.finalize())))
                .map_err(|err| {
                    OperationError::new(
                        OperationErrorCode::IoFailed,
                        format!("failed to read installed Battery script: {err}"),
                    )
                });
        }
        match installed.read_exact(&mut installed_buffer[..count]) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(err) => {
                return Err(OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("failed to read installed Battery script: {err}"),
                ));
            }
        }
        if source_buffer[..count] != installed_buffer[..count] {
            return Ok(None);
        }
        source_hash.update(&source_buffer[..count]);
    }
}

pub(super) fn open_validated_script_entry(
    cache_path: &Path,
    script: &BatteryManifestScript,
) -> OperationResult<(PathBuf, File)> {
    if script.id.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::ManifestInvalid,
            "battery script id is required",
        ));
    }
    reject_unsafe_relative_path(&script.path)?;
    reject_reserved_install_path(&script.path)?;
    if script_kind(&script.path).is_none() {
        return Err(OperationError::new(
            OperationErrorCode::UnsupportedScript,
            format!(
                "unsupported battery script extension: {}",
                script.path.display()
            ),
        ));
    }

    // Check the raw joined path first: canonicalize follows symlinks, which
    // would hide the fact that the manifest pointed at a link.
    reject_symlink_components(cache_path, &script.path, true)?;
    let full_path = confined_existing_path(cache_path, &script.path)?;
    verify_tracked_blob(cache_path, &script.path)?;
    let mut file = open_existing_file_no_follow(&full_path)?;
    validate_script_schema_from_file(&script.path, &mut file)?;
    file.seek(SeekFrom::Start(0)).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to rewind battery script: {err}"),
        )
    })?;
    Ok((full_path, file))
}

fn validate_script_schema_from_file(path: &Path, file: &mut File) -> OperationResult<()> {
    let prefixes = match script_kind(path) {
        Some(ScriptKind::Bash) => vec!["#"],
        Some(ScriptKind::PowerShell) => vec!["#", ";"],
        Some(ScriptKind::Python) => vec!["#"],
        Some(ScriptKind::Lua) => vec!["--"],
        None => {
            return Err(OperationError::new(
                OperationErrorCode::UnsupportedScript,
                format!("unsupported battery script extension: {}", path.display()),
            ));
        }
    };
    let mut contents = String::new();
    file.seek(SeekFrom::Start(0)).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to rewind battery script: {err}"),
        )
    })?;
    file.read_to_string(&mut contents).map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to read battery script: {err}"),
        )
    })?;
    let block = extract_schema_block(&contents, &prefixes).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("battery script schema block is invalid: {err}"),
        )
    })?;
    parse_schema(&block).map_err(|err| {
        OperationError::new(
            OperationErrorCode::ManifestInvalid,
            format!("battery script schema is invalid: {err}"),
        )
    })?;
    Ok(())
}
