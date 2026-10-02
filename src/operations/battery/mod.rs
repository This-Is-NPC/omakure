mod files;
#[cfg(unix)]
mod fs_unix;
mod git;
mod git_url;
mod install;
mod manifest;
mod path_safety;
mod registry;
mod sync;
mod types;

pub use git::{GitCommandSpec, git_checkout_detached_spec, git_clone_spec, git_fetch_spec};
pub use git_url::{
    assert_git_url_host_public_literal, assert_local_battery_allowed, assert_public_git_host,
};
pub use install::install_battery_script;
pub(crate) use install::{InstallState, install_verified_script};
pub use manifest::{
    BatteryManifest, BatteryManifestHeader, BatteryManifestScript, MANIFEST_FILE, load_manifest,
    parse_manifest, validate_manifest, validate_script_entry,
};
pub use path_safety::{confined_existing_path, reject_unsafe_relative_path};
pub use registry::{
    BatteryPaths, REGISTRY_VERSION, add_battery, inspect_battery, installing_battery,
    list_batteries, list_battery_scripts, read_registry, remove_battery, write_registry,
};
pub use sync::{sync_battery, sync_battery_https_only_with_access};
pub use types::{
    AddBatteryRequest, BatteryAuth, BatteryAuthMethod, BatteryCacheStatus, BatteryInspectResponse,
    BatteryRegistry, BatteryScriptSummary, BatterySummary, InspectBatteryRequest,
    InstallBatteryScriptRequest, InstallBatteryScriptResponse, RemoveBatteryRequest,
    RemoveBatteryResponse, SyncBatteryRequest,
};

#[cfg(test)]
mod tests;
