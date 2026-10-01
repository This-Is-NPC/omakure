use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub version: String,
    pub workspace_root: PathBuf,
    pub scripts_root: PathBuf,
    pub omakure_dir: PathBuf,
    pub history_dir: PathBuf,
    pub workspace_config: PathBuf,
    pub envs_dir: PathBuf,
    pub envs_active_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListScriptsRequest {
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSummary {
    pub absolute_path: String,
    pub relative_path: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub field_count: usize,
    pub schema_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribeScriptRequest {
    pub script: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptDescription {
    pub absolute_path: String,
    pub relative_path: String,
    pub schema: ScriptSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSchema {
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub fields: Vec<ScriptField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptField {
    pub name: String,
    pub prompt: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub order: u32,
    pub required: bool,
    pub arg: Option<String>,
    pub default: Option<String>,
    pub choices: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListRunsRequest {
    pub script: Option<String>,
    pub actor: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub success: Option<bool>,
    pub limit: Option<i64>,
    pub states: Vec<String>,
    pub state_set: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowRunRequest {
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTracesRequest {
    pub run_id: String,
    pub level: Option<String>,
    pub since_sequence: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnqueueRunRequest {
    pub script: String,
    pub args: Vec<String>,
    pub env: Option<String>,
    pub secret_fields: Vec<(String, String)>,
    pub run_id: Option<String>,
    pub actor: String,
    pub reason: Option<String>,
    pub priority: i64,
    pub timeout_ms: Option<i64>,
    pub parent_run_id: Option<String>,
    pub cron_schedule_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelRunRequest {
    pub run_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadLetterRunRequest {
    pub run_id: String,
    pub reason: Option<String>,
}
