use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::files::open_existing_file_no_follow;
use super::install::verify_tracked_blob;
use super::path_safety::{
    confined_existing_path, reject_reserved_install_path, reject_symlink_components,
    reject_unsafe_relative_path,
};
use crate::domain::{extract_schema_block, parse_schema};
use crate::runtime::{ScriptKind, script_kind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "omakure-battery.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryManifest {
    pub battery: BatteryManifestHeader,
    #[serde(default)]
    pub scripts: Vec<BatteryManifestScript>,
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
