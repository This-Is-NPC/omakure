use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub catalog_version: String,
    #[serde(default)]
    pub operations: Vec<Operation>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub operation_id: String,
    pub entry_id: String,
    pub plane: Plane,
    pub remote_eligibility: RemoteEligibility,
    pub effect: Effect,
    pub mutability: Mutability,
    #[serde(default)]
    pub cli: Vec<String>,
    #[serde(default)]
    pub http: Vec<String>,
    pub platforms: PlatformSupportSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Plane {
    Domain,
    LocalLifecycle,
    ServiceObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteEligibility {
    LocalOnly,
    ControlObserve,
    ControlExecute,
    ControlConverge,
    FutureContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Read,
    Observe,
    Mutate,
    Execute,
    Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mutability {
    Immutable,
    Idempotent,
    NonIdempotent,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSupportSet {
    pub linux: PlatformSupport,
    pub macos: PlatformSupport,
    pub windows: PlatformSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSupport {
    pub supported: bool,
    pub reason: String,
}
