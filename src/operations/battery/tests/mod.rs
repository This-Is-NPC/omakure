use super::super::{OperationError, OperationErrorCode};
use super::git::{
    assert_git_http_pinning_supported_with, git_command, git_command_with_context,
    prepare_git_askpass, prepare_git_config_in, reject_unsafe_git_config_text, sanitize_git_output,
    sanitize_git_stderr, GitExecContext, GitHttpPin, GitTransportPolicy,
};
use super::git_url::{
    normalize_git_url, resolve_public_git_endpoint, strip_windows_verbatim_owned, validate_git_ref,
    validate_git_url,
};
#[cfg(unix)]
use super::registry::cache_path_for_battery;
use super::*;
use crate::secrets::SecretAccess;
use crate::test_support::{run_git, workspace_in};
use crate::util::hex;
use crate::workspace::Workspace;
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

mod git;
mod git_url;
mod inspect;
mod install;
mod manifest;
mod path_safety;
mod registry;
mod sync;

fn env_key_eq(key: &std::ffi::OsStr, expected: &str) -> bool {
    #[cfg(windows)]
    {
        key.to_string_lossy().eq_ignore_ascii_case(expected)
    }
    #[cfg(not(windows))]
    {
        key == std::ffi::OsStr::new(expected)
    }
}

fn valid_schema_script() -> String {
    r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {
#   "Name": "battery_script",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END
echo ok
"#
    .to_string()
}

fn write_manifest_and_script(cache: &Path) {
    fs::create_dir_all(cache.join("scripts")).unwrap();
    fs::write(cache.join("scripts/list.sh"), valid_schema_script()).unwrap();
    fs::write(
        cache.join(MANIFEST_FILE),
        r#"
[battery]
name = "azure"
version = "0.1.0"
description = "Azure scripts"

[[scripts]]
id = "azure.list"
path = "scripts/list.sh"
description = "List"
tags = ["azure"]
"#,
    )
    .unwrap();
}

fn synced_registry_with_commit(commit: impl Into<String>) -> BatteryRegistry {
    BatteryRegistry {
        version: REGISTRY_VERSION,
        batteries: vec![BatterySummary {
            name: "azure".into(),
            git_url: "https://example.invalid/azure.git".into(),
            requested_ref: "main".into(),
            resolved_commit: Some(commit.into()),
            cache_path: PathBuf::from(".omakure/batteries/cache/azure"),
            last_synced_at: Some("2026-07-07T00:00:00Z".into()),
            auth: None,
        }],
    }
}

fn cache_head(cache: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cache)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_cache_git(cache: &Path) -> String {
    run_git(&["init", "-b", "main"], cache);
    run_git(&["add", "."], cache);
    run_git(
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "initial",
        ],
        cache,
    );
    cache_head(cache)
}

fn write_synced_cache_and_registry(ws: &Workspace) -> PathBuf {
    let paths = BatteryPaths::for_workspace(ws);
    let cache = paths.cache_path_for("azure");
    write_manifest_and_script(&cache);
    let commit = init_cache_git(&cache);
    write_registry(&paths.registry_path, &synced_registry_with_commit(commit)).unwrap();
    cache
}

fn create_battery_repo() -> TempDir {
    let repo = TempDir::new().unwrap();
    run_git(&["init", "-b", "main"], repo.path());
    write_manifest_and_script(repo.path());
    run_git(&["add", "."], repo.path());
    run_git(
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "initial",
        ],
        repo.path(),
    );
    repo
}
