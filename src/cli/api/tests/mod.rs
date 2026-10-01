use super::audit::{HttpAuditEvent, clear_audit_hook, install_audit_hook};
use super::boot::{ApiConfigError, prepare_api_boot, validate_bind};
use super::respond::operation_error_response;
use super::router::{
    BODY_LIMIT_BYTES, router, router_with_auth, router_with_deploy, router_with_health_plane,
    router_with_policy, shared_test_health_registry,
};
use super::scripts::{MAX_SEARCH_QUERY_LEN, MAX_SEARCH_TAG_LEN, MAX_SEARCH_TAGS};
use super::state::{ApiPolicy, ReadinessGate};
use crate::app_meta;
use crate::auth::{self, AuthContext, Authenticator, test_credential};
use crate::cli::args::ApiArgs;
use crate::inventory::HTTP_ROUTE_INVENTORY;
use crate::operations::battery as battery_ops;
use crate::operations::envs as env_ops;
use crate::operations::{OperationError, OperationErrorCode};
use crate::policy::DeployPolicy;
use crate::workspace::Workspace;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use std::net::SocketAddr;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use tempfile::TempDir;
use tower::ServiceExt;

mod audit;
mod battery;
mod bearer;
mod boot;
mod capabilities;
mod envs;
mod node;
mod respond;
mod router;
mod runs;
mod scripts;
mod secrets;
mod status;

/// Test sink for HTTP audit events (installs process-wide hook).
/// Serialized so parallel tokio tests do not clobber the global hook.
struct AuditCapture {
    events: Arc<Mutex<Vec<HttpAuditEvent>>>,
    _permit: tokio::sync::OwnedMutexGuard<()>,
}

impl AuditCapture {
    async fn install() -> Self {
        static AUDIT_TEST_LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
        let permit = AUDIT_TEST_LOCK
            .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
            .lock_owned()
            .await;
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        install_audit_hook(Arc::new(move |event: &HttpAuditEvent| {
            sink.lock().expect("audit lock").push(event.clone());
        }));
        Self {
            events,
            _permit: permit,
        }
    }

    fn events(&self) -> Vec<HttpAuditEvent> {
        self.events.lock().expect("audit lock").clone()
    }
}

impl Drop for AuditCapture {
    fn drop(&mut self) {
        clear_audit_hook();
    }
}

async fn response_json(response: Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), BODY_LIMIT_BYTES)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn write_script(root: &std::path::Path, name: &str) {
    if let Some(parent) = root.join(name).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        root.join(name),
        format!(
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {{\"Name\":\"{name}\",\"Description\":\"test script\",\"Tags\":[\"ops\"],\"Fields\":[]}}\n# OMAKURE_SCHEMA_END\necho ok\n"
        ),
    )
    .unwrap();
}

fn write_secret_script(root: &std::path::Path, name: &str, default: Option<&str>) {
    let default_line = default
        .map(|value| format!(r#", "Default":"{value}""#))
        .unwrap_or_default();
    std::fs::write(
        root.join(name),
        format!(
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {{"Name":"Secret","Fields":[{{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"{default_line}}}]}}
# OMAKURE_SCHEMA_END
echo ok
"#
        ),
    )
    .unwrap();
}

fn authed_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", test_credential::token()),
        )
        .body(Body::empty())
        .unwrap()
}

fn authed_json_request(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", test_credential::token()),
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authed_json_method_request(method: Method, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", test_credential::token()),
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authed_delete_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", test_credential::token()),
        )
        .body(Body::empty())
        .unwrap()
}

fn invalid_manifest_repo() -> TempDir {
    let repo = TempDir::new().unwrap();
    crate::test_support::run_git(&["init", "-b", "main"], repo.path());
    std::fs::write(repo.path().join("omakure-battery.toml"), "not = [valid").unwrap();
    crate::test_support::run_git(&["add", "."], repo.path());
    crate::test_support::run_git(
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "invalid manifest",
        ],
        repo.path(),
    );
    repo
}

fn register_invalid_https_battery_cache(workspace: &Workspace, name: &str) {
    let paths = battery_ops::BatteryPaths::for_workspace(workspace);
    let cache = paths.cache_path_for(name);
    std::fs::create_dir_all(&cache).unwrap();
    crate::test_support::run_git(&["init", "-b", "main"], &cache);
    std::fs::write(cache.join("omakure-battery.toml"), "not = [valid").unwrap();
    crate::test_support::run_git(&["add", "."], &cache);
    crate::test_support::run_git(
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "invalid manifest",
        ],
        &cache,
    );
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&cache)
        .output()
        .unwrap();
    assert!(head.status.success());
    let registry = battery_ops::BatteryRegistry {
        version: battery_ops::REGISTRY_VERSION,
        batteries: vec![battery_ops::BatterySummary {
            name: name.to_string(),
            git_url: format!("https://example.invalid/{name}.git"),
            requested_ref: "main".into(),
            resolved_commit: Some(String::from_utf8_lossy(&head.stdout).trim().to_string()),
            cache_path: paths
                .cache_path_for(name)
                .strip_prefix(workspace.root())
                .unwrap()
                .to_path_buf(),
            last_synced_at: Some("2026-07-07T00:00:00Z".into()),
            auth: None,
        }],
    };
    std::fs::write(
        &paths.registry_path,
        serde_json::to_string_pretty(&registry).unwrap(),
    )
    .unwrap();
}
