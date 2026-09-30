use super::manifest::BatteryManifest;
use super::registry::REGISTRY_VERSION;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How a Battery authenticates to a private HTTPS remote.
///
/// Registry stores method + secret ref only — never resolved plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryAuthMethod {
    HttpsTokenRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryAuth {
    pub method: BatteryAuthMethod,
    /// Canonical `secret://provider/key` ref. Never a plaintext token.
    pub token_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatterySummary {
    pub name: String,
    pub git_url: String,
    pub requested_ref: String,
    pub resolved_commit: Option<String>,
    pub cache_path: PathBuf,
    pub last_synced_at: Option<String>,
    /// Present when the Battery uses private HTTPS auth via a secret ref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<BatteryAuth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryRegistry {
    pub version: u32,
    pub batteries: Vec<BatterySummary>,
}

impl Default for BatteryRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            batteries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryScriptSummary {
    pub id: String,
    pub path: PathBuf,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectBatteryRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryInspectResponse {
    pub summary: BatterySummary,
    pub manifest: BatteryManifest,
    pub cache_status: BatteryCacheStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryCacheStatus {
    NotSynced,
    Synced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddBatteryRequest {
    pub name: String,
    pub git_url: String,
    pub requested_ref: String,
    /// Optional `secret://…` ref for private HTTPS clone/fetch (GIT_ASKPASS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallBatteryScriptRequest {
    pub battery_name: String,
    pub script_id: String,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncBatteryRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallBatteryScriptResponse {
    pub installed_path: PathBuf,
    pub provenance_path: PathBuf,
    pub battery_name: String,
    pub script_id: String,
    pub resolved_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveBatteryResponse {
    pub name: String,
    pub cache_removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct InstalledScriptProvenance {
    pub(super) battery_name: String,
    pub(super) script_id: String,
    pub(super) git_url: String,
    pub(super) requested_ref: String,
    pub(super) resolved_commit: String,
    pub(super) source_path: PathBuf,
    pub(super) installed_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveBatteryRequest {
    pub name: String,
    pub remove_cache: bool,
}
