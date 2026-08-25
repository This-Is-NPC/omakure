//! Deploy-only `policy.toml` for `omakure api` / `omakure node serve`.
//!
//! Separate from workspace `omakure.toml`. Load via `--policy` /
//! `OMAKURE_POLICY_FILE`. Route-group gates apply **before** token scopes.
//!
//! ## Load order (api / node serve)
//!
//! 1. Built-in defaults (permissive route groups; node workers=1, scheduler on).
//! 2. Deploy `policy.toml` overlays http/node defaults and hard route gates.
//! 3. Explicit CLI flags override policy for bind / workers / scheduler /
//!    readiness / tokens-file / allow-non-loopback when provided.
//! 4. Workspace `omakure.toml` is never consulted for deploy policy.

use serde::Deserialize;
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct DeployPolicy {
    pub version: u32,
    pub http: HttpPolicy,
    pub node: NodeServicePolicy,
    pub routes: RoutesPolicy,
    pub sources: SourcesPolicy,
    pub scripts: ScriptsPolicy,
    pub envs: EnvsPolicy,
    pub runs: RunsPolicy,
    pub secrets: SecretsPolicy,
    pub auth: AuthPolicy,
}

impl Default for DeployPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            http: HttpPolicy::default(),
            node: NodeServicePolicy::default(),
            routes: RoutesPolicy::default(),
            sources: SourcesPolicy::default(),
            scripts: ScriptsPolicy::default(),
            envs: EnvsPolicy::default(),
            runs: RunsPolicy::default(),
            secrets: SecretsPolicy::default(),
            auth: AuthPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpPolicy {
    pub enabled: bool,
    pub bind: Option<SocketAddr>,
    pub allow_non_loopback: bool,
    pub body_limit_bytes: usize,
    pub cors: String,
}

impl Default for HttpPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: None,
            allow_non_loopback: false,
            body_limit_bytes: 1_048_576,
            cors: "disabled".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeServicePolicy {
    pub workers: Option<u32>,
    pub readiness_requires_worker: bool,
    pub scheduler: Option<bool>,
    pub readiness_requires_scheduler: bool,
    pub readiness_requires_transport: bool,
    pub allow_non_loopback_direct: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoutesPolicy {
    pub read: bool,
    pub writes: bool,
    pub node: bool,
    pub trust: bool,
    pub enrollment: bool,
    pub battery: bool,
    pub battery_install: bool,
    pub run_enqueue: bool,
    pub run_cancel: bool,
    pub run_dead_letter: bool,
    pub config: bool,
    pub doctor: bool,
    pub envs: bool,
}

impl Default for RoutesPolicy {
    fn default() -> Self {
        Self {
            read: true,
            writes: true,
            node: true,
            trust: true,
            enrollment: true,
            battery: true,
            battery_install: true,
            run_enqueue: true,
            run_cancel: true,
            run_dead_letter: true,
            config: true,
            doctor: true,
            envs: true,
        }
    }
}

/// Battery source gates. SSH Battery sources are not supported over the HTTP
/// API/node service surface (the add handler rejects non-`https` URLs), so there is
/// no SSH toggle here.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourcesPolicy {
    pub allow_https_batteries: bool,
    pub allow_local_batteries: bool,
    pub allow_private_https_batteries: bool,
}

impl Default for SourcesPolicy {
    fn default() -> Self {
        Self {
            allow_https_batteries: true,
            allow_local_batteries: true,
            allow_private_https_batteries: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScriptsPolicy {
    pub max_content_bytes: usize,
    pub tree_entry_limit: usize,
}

impl Default for ScriptsPolicy {
    fn default() -> Self {
        Self {
            max_content_bytes: 1_048_576,
            tree_entry_limit: 1000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnvsPolicy {
    pub http_manage: bool,
    pub allow_secret_refs: bool,
}

impl Default for EnvsPolicy {
    fn default() -> Self {
        Self {
            http_manage: true,
            allow_secret_refs: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunsPolicy {
    pub allow_env_selection: bool,
    pub allow_secret_fields: bool,
}

impl Default for RunsPolicy {
    fn default() -> Self {
        Self {
            allow_env_selection: true,
            allow_secret_fields: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecretsPolicy {
    pub provider: String,
    pub metadata_endpoint: bool,
}

impl Default for SecretsPolicy {
    fn default() -> Self {
        Self {
            provider: "file".to_string(),
            metadata_endpoint: false,
        }
    }
}

/// Default `[auth] max_concurrent_verifications`: two concurrent 64 MiB
/// Argon2id verifications keep the authentication memory budget near 128 MiB
/// while excess requests fail fast instead of forming an unbounded queue.
pub const DEFAULT_MAX_CONCURRENT_AUTH_VERIFICATIONS: usize = 2;
/// Maximum configurable authentication memory budget: eight concurrent
/// Argon2id verifications at roughly 64 MiB each (about 512 MiB total).
pub const MAX_CONCURRENT_AUTH_VERIFICATIONS: usize = 8;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthPolicy {
    pub tokens_file: Option<PathBuf>,
    pub legacy_env_token: bool,
    /// Concurrency bound on in-flight Argon2id bearer verifications. Each
    /// verify is memory-hard (~64 MiB); this trades authentication
    /// availability against memory/CPU exhaustion. A tighter bound is easier
    /// to exhaust with concurrent requests against a single known token id
    /// (id existence is not secret — it is visible in the bearer string
    /// itself); a looser bound raises the worst-case memory footprint
    /// (`max_concurrent_verifications * ~64 MiB`). Tune per deployment.
    pub max_concurrent_verifications: usize,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            tokens_file: None,
            // Default true preserves pre-policy legacy OMAKURE_API_TOKEN behavior.
            legacy_env_token: true,
            max_concurrent_verifications: DEFAULT_MAX_CONCURRENT_AUTH_VERIFICATIONS,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeployPolicyToml {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    http: HttpPolicy,
    #[serde(default)]
    node: NodeServicePolicy,
    #[serde(default)]
    routes: RoutesPolicy,
    #[serde(default)]
    sources: SourcesPolicy,
    #[serde(default)]
    scripts: ScriptsPolicy,
    #[serde(default)]
    envs: EnvsPolicy,
    #[serde(default)]
    runs: RunsPolicy,
    #[serde(default)]
    secrets: SecretsPolicy,
    #[serde(default)]
    auth: AuthPolicy,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    Io(String),
    Parse(String),
    UnsupportedVersion(u32),
    Invalid(String),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "policy file I/O error: {msg}"),
            Self::Parse(msg) => write!(f, "policy file parse error: {msg}"),
            Self::UnsupportedVersion(v) => {
                write!(f, "unsupported policy file version: {v} (expected 1)")
            }
            Self::Invalid(msg) => write!(f, "invalid policy: {msg}"),
        }
    }
}

impl std::error::Error for PolicyError {}

/// Resolve policy path from CLI / env. Missing path → built-in defaults.
pub fn resolve_policy_path(cli_path: Option<&Path>, env_path: Option<&str>) -> Option<PathBuf> {
    cli_path
        .map(Path::to_path_buf)
        .or_else(|| env_path.map(PathBuf::from))
}

/// Load deploy policy. `None` path returns permissive defaults (no file).
pub fn load_policy(path: Option<&Path>) -> Result<DeployPolicy, PolicyError> {
    match path {
        None => Ok(DeployPolicy::default()),
        Some(path) => {
            let text = fs::read_to_string(path).map_err(|e| PolicyError::Io(e.to_string()))?;
            parse_policy_toml(&text)
        }
    }
}

pub fn parse_policy_toml(text: &str) -> Result<DeployPolicy, PolicyError> {
    let parsed: DeployPolicyToml =
        toml::from_str(text).map_err(|e| PolicyError::Parse(e.to_string()))?;
    if parsed.version != 1 {
        return Err(PolicyError::UnsupportedVersion(parsed.version));
    }
    if parsed.http.body_limit_bytes == 0 {
        return Err(PolicyError::Invalid(
            "http.body_limit_bytes must be > 0".into(),
        ));
    }
    if parsed.http.cors != "disabled" {
        return Err(PolicyError::Invalid(format!(
            "http.cors = {:?} is unsupported in v1 (only \"disabled\")",
            parsed.http.cors
        )));
    }
    if !parsed.http.enabled {
        return Err(PolicyError::Invalid(
            "http.enabled = false is unsupported for api/node serve (omit the process instead)"
                .into(),
        ));
    }
    if parsed.auth.max_concurrent_verifications == 0 {
        return Err(PolicyError::Invalid(
            "auth.max_concurrent_verifications must be > 0".into(),
        ));
    }
    if parsed.auth.max_concurrent_verifications > MAX_CONCURRENT_AUTH_VERIFICATIONS {
        return Err(PolicyError::Invalid(format!(
            "auth.max_concurrent_verifications must be <= {MAX_CONCURRENT_AUTH_VERIFICATIONS}"
        )));
    }
    Ok(DeployPolicy {
        version: parsed.version,
        http: parsed.http,
        node: parsed.node,
        routes: parsed.routes,
        sources: parsed.sources,
        scripts: parsed.scripts,
        envs: parsed.envs,
        runs: parsed.runs,
        secrets: parsed.secrets,
        auth: parsed.auth,
    })
}

impl RoutesPolicy {
    /// Whether the HTTP method+path is allowed by deploy route groups.
    /// Checked before token scopes.
    pub fn allows(&self, method: &str, path: &str) -> bool {
        let method = method.to_ascii_uppercase();
        let is_write = matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
        let is_battery = path == "/v1/batteries" || path.starts_with("/v1/batteries/");
        let is_node = path == "/v1/node" || path.starts_with("/v1/node/");
        let is_trust = is_node && path.contains("/peers");
        let is_enrollment = is_node && path.contains("/enrollment");

        if is_node && !self.node {
            return false;
        }
        if is_trust && is_write && !self.trust {
            return false;
        }
        if is_enrollment && is_write && !self.enrollment {
            return false;
        }
        if is_battery && !self.battery {
            return false;
        }
        if is_write && !self.writes {
            return false;
        }
        if is_battery
            && method == "POST"
            && path.contains("/scripts/")
            && path.ends_with("/install")
            && !self.battery_install
        {
            return false;
        }
        if method == "POST" && path == "/v1/runs" && !self.run_enqueue {
            return false;
        }
        if method == "POST"
            && path.ends_with("/cancel")
            && path.starts_with("/v1/runs/")
            && !self.run_cancel
        {
            return false;
        }
        if method == "POST"
            && path.ends_with("/dead-letter")
            && path.starts_with("/v1/runs/")
            && !self.run_dead_letter
        {
            return false;
        }
        if !self.config
            && (matches!(
                path,
                "/v1/config" | "/v1/workspace" | "/v1/search" | "/v1/tree"
            ) || path.starts_with("/v1/tree/"))
        {
            return false;
        }
        if !self.doctor && path == "/v1/doctor" {
            return false;
        }
        if !self.envs && (path == "/v1/envs" || path.starts_with("/v1/envs/")) {
            return false;
        }
        if !is_write
            && !self.read
            && path != "/v1/health"
            && path != "/v1/ready"
            && path != "/v1/admin/status"
        {
            // Health, readiness, and admin status are observability endpoints and
            // must survive a read-group lockdown (they still require the
            // `admin:status` token scope). Battery already handled; the rest are
            // the "read" group.
            if !is_battery {
                return false;
            }
        }
        true
    }
}

impl DeployPolicy {
    pub fn deny_reason(&self, method: &str, path: &str) -> Option<&'static str> {
        if self.routes.allows(method, path) {
            None
        } else {
            Some("deployment policy denies this route")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn default_policy_allows_writes_and_battery() {
        let p = DeployPolicy::default();
        assert!(p.routes.writes);
        assert!(p.routes.battery);
        assert!(p.auth.legacy_env_token);
        assert!(p.routes.allows("POST", "/v1/runs"));
        assert!(p.routes.allows("GET", "/v1/batteries"));
    }

    #[test]
    fn read_false_still_allows_observability_endpoints() {
        let mut p = DeployPolicy::default();
        p.routes.read = false;
        // Monitoring survives a read lockdown (scope still enforced downstream).
        assert!(p.routes.allows("GET", "/v1/health"));
        assert!(p.routes.allows("GET", "/v1/ready"));
        assert!(p.routes.allows("GET", "/v1/admin/status"));
        // A normal read route is still denied.
        assert!(!p.routes.allows("GET", "/v1/scripts"));
    }

    #[test]
    fn parse_rejects_bad_toml() {
        let err = parse_policy_toml("not = [valid").unwrap_err();
        assert!(matches!(err, PolicyError::Parse(_)));
    }

    #[test]
    fn parse_rejects_unsupported_version() {
        let err = parse_policy_toml("version = 99\n").unwrap_err();
        assert_eq!(err, PolicyError::UnsupportedVersion(99));
    }

    #[test]
    fn writes_false_denies_all_write_methods() {
        let mut p = DeployPolicy::default();
        p.routes.writes = false;
        assert!(!p.routes.allows("POST", "/v1/runs"));
        assert!(!p.routes.allows("PUT", "/v1/envs/dev"));
        assert!(!p.routes.allows("PATCH", "/v1/envs/dev"));
        assert!(!p.routes.allows("DELETE", "/v1/envs/dev"));
        assert!(p.routes.allows("GET", "/v1/scripts"));
        assert!(p.routes.allows("GET", "/v1/runs"));
    }

    #[test]
    fn battery_false_denies_all_battery_paths() {
        let mut p = DeployPolicy::default();
        p.routes.battery = false;
        assert!(!p.routes.allows("GET", "/v1/batteries"));
        assert!(!p.routes.allows("POST", "/v1/batteries"));
        assert!(!p.routes.allows("GET", "/v1/batteries/azure"));
        assert!(!p.routes.allows("DELETE", "/v1/batteries/azure"));
        assert!(!p.routes.allows("POST", "/v1/batteries/azure/sync"));
        assert!(p.routes.allows("GET", "/v1/scripts"));
    }

    #[test]
    fn node_and_trust_route_groups_are_independently_gated() {
        let mut p = DeployPolicy::default();
        for (method, path) in [
            ("GET", "/v1/node/status"),
            ("POST", "/v1/node/init"),
            ("GET", "/v1/node/peers"),
            ("POST", "/v1/node/peers"),
            ("PATCH", "/v1/node/peers/omk1_test/capabilities"),
            ("POST", "/v1/node/peers/omk1_test/revoke"),
        ] {
            assert!(
                p.routes.allows(method, path),
                "default policy: {method} {path}"
            );
        }
        p.routes.node = false;
        for (method, path) in [
            ("GET", "/v1/node/status"),
            ("POST", "/v1/node/init"),
            ("GET", "/v1/node/peers"),
            ("POST", "/v1/node/peers"),
            ("PATCH", "/v1/node/peers/omk1_test/capabilities"),
            ("POST", "/v1/node/peers/omk1_test/revoke"),
        ] {
            assert!(
                !p.routes.allows(method, path),
                "node disabled: {method} {path}"
            );
        }

        p.routes.node = true;
        p.routes.trust = false;
        assert!(p.routes.allows("GET", "/v1/node/peers"));
        assert!(!p.routes.allows("POST", "/v1/node/peers"));
        assert!(!p
            .routes
            .allows("PATCH", "/v1/node/peers/omk1_test/capabilities"));
        assert!(!p.routes.allows("POST", "/v1/node/peers/omk1_test/revoke"));
    }

    #[test]
    fn enrollment_route_group_is_independent_from_trust() {
        let mut p = DeployPolicy::default();
        assert!(p.routes.allows("GET", "/v1/node/enrollments"));
        assert!(p.routes.allows("POST", "/v1/node/enrollments"));
        p.routes.enrollment = false;
        assert!(p.routes.allows("GET", "/v1/node/enrollments"));
        assert!(!p.routes.allows("POST", "/v1/node/enrollments"));
        assert!(!p
            .routes
            .allows("POST", "/v1/node/enrollments/omk1_test/approve"));
        p.routes.enrollment = true;
        p.routes.trust = false;
        assert!(p
            .routes
            .allows("POST", "/v1/node/enrollments/omk1_test/approve"));
    }

    #[test]
    fn parse_full_v1_schema_example() {
        let text = r#"
version = 1

[http]
enabled = true
bind = "0.0.0.0:7878"
allow_non_loopback = true
body_limit_bytes = 1048576
cors = "disabled"

[node]
workers = 2
readiness_requires_worker = true
scheduler = true
readiness_requires_scheduler = true

[routes]
read = true
writes = false
battery = false
battery_install = false
run_enqueue = true
run_cancel = true
run_dead_letter = false
config = true
doctor = true
envs = true

[sources]
allow_https_batteries = true
allow_local_batteries = false
allow_private_https_batteries = true

[scripts]
max_content_bytes = 1048576
tree_entry_limit = 1000

[envs]
http_manage = true
allow_secret_refs = true

[runs]
allow_env_selection = true
allow_secret_fields = true

[secrets]
provider = "file"
metadata_endpoint = false

[auth]
tokens_file = "/run/secrets/omakure_tokens.toml"
legacy_env_token = false
max_concurrent_verifications = 4
"#;
        let p = parse_policy_toml(text).unwrap();
        assert_eq!(p.version, 1);
        assert!(!p.routes.writes);
        assert!(!p.routes.battery);
        assert!(!p.auth.legacy_env_token);
        assert_eq!(p.node.workers, Some(2));
        assert_eq!(p.node.scheduler, Some(true));
        assert_eq!(
            p.auth.tokens_file.as_deref(),
            Some(Path::new("/run/secrets/omakure_tokens.toml"))
        );
        assert_eq!(p.auth.max_concurrent_verifications, 4);
        assert_eq!(p.http.bind, Some("0.0.0.0:7878".parse().unwrap()));
        assert!(p.http.allow_non_loopback);
    }

    #[test]
    fn default_max_concurrent_verifications_matches_constant() {
        assert_eq!(
            DeployPolicy::default().auth.max_concurrent_verifications,
            DEFAULT_MAX_CONCURRENT_AUTH_VERIFICATIONS
        );
    }

    #[test]
    fn parse_rejects_zero_max_concurrent_verifications() {
        let text = "version = 1\n[auth]\nmax_concurrent_verifications = 0\n";
        let err = parse_policy_toml(text).unwrap_err();
        assert!(matches!(err, PolicyError::Invalid(_)));
        assert!(err.to_string().contains("max_concurrent_verifications"));
    }

    #[test]
    fn parse_accepts_max_concurrent_verifications_limit() {
        let text = format!(
            "version = 1\n[auth]\nmax_concurrent_verifications = {MAX_CONCURRENT_AUTH_VERIFICATIONS}\n"
        );
        let policy = parse_policy_toml(&text).unwrap();

        assert_eq!(
            policy.auth.max_concurrent_verifications,
            MAX_CONCURRENT_AUTH_VERIFICATIONS
        );
    }

    #[test]
    fn parse_rejects_max_concurrent_verifications_above_limit() {
        let value = MAX_CONCURRENT_AUTH_VERIFICATIONS + 1;
        let text = format!("version = 1\n[auth]\nmax_concurrent_verifications = {value}\n");
        let err = parse_policy_toml(&text).unwrap_err();

        assert!(matches!(err, PolicyError::Invalid(_)));
        assert!(err
            .to_string()
            .contains(&format!("must be <= {MAX_CONCURRENT_AUTH_VERIFICATIONS}")));
    }

    #[test]
    fn parse_rejects_unknown_top_level_key() {
        let err = parse_policy_toml("version = 1\nroutez = {}\n").unwrap_err();
        assert!(matches!(err, PolicyError::Parse(_)));
        assert!(err.to_string().contains("routez"));
    }

    #[test]
    fn parse_rejects_unknown_key_in_every_policy_section() {
        for section in [
            "http", "node", "routes", "sources", "scripts", "envs", "runs", "secrets", "auth",
        ] {
            let text = format!("version = 1\n[{section}]\nunknown_policy_key = true\n");
            let err = parse_policy_toml(&text).unwrap_err();
            assert!(
                matches!(err, PolicyError::Parse(_)),
                "section [{section}] accepted an unknown field"
            );
            assert!(err.to_string().contains("unknown_policy_key"));
        }
    }

    #[test]
    fn removed_ssh_source_key_is_rejected_fail_closed() {
        let text = "version = 1\n[sources]\nallow_private_ssh_batteries = true\n";
        let err = parse_policy_toml(text).unwrap_err();
        assert!(matches!(err, PolicyError::Parse(_)));
        assert!(err.to_string().contains("allow_private_ssh_batteries"));
    }

    #[test]
    fn load_missing_file_is_io_error() {
        let err = load_policy(Some(Path::new("/no/such/policy.toml"))).unwrap_err();
        assert!(matches!(err, PolicyError::Io(_)));
    }

    #[test]
    fn load_none_returns_defaults() {
        let p = load_policy(None).unwrap();
        assert_eq!(p, DeployPolicy::default());
    }

    #[test]
    fn load_valid_file() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "version = 1\n[routes]\nwrites = false\n").unwrap();
        let p = load_policy(Some(f.path())).unwrap();
        assert!(!p.routes.writes);
    }

    #[test]
    fn resolve_policy_path_prefers_cli_over_env() {
        let cli = Path::new("/cli/policy.toml");
        let resolved = resolve_policy_path(Some(cli), Some("/env/policy.toml"));
        assert_eq!(resolved.as_deref(), Some(cli));
    }
}
