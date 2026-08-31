//! Manifest-complete paired CLI/HTTP behavioral parity harness.
//!
//! Family modules own real probes; this file owns deterministic fixture setup,
//! adapter invocation and registry aggregation.

mod support;

#[path = "behavioral_parity/battery.rs"]
mod battery;
#[path = "behavioral_parity/core.rs"]
mod core;
#[path = "behavioral_parity/env_history_queue.rs"]
mod env_history_queue;
#[path = "behavioral_parity/node.rs"]
mod node;

use omakure::cli_http_parity::{ProbeEvidence, ProbeFixture};
use serde_json::Value;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

pub const API_TOKEN: &str = "behavioral-parity-token-000000000000000000000000";

pub struct BehavioralContext {
    pub workspace: support::TestWorkspace,
    pub repository: support::TestWorkspace,
    pub server: support::HttpServer,
    pub fixture: ProbeFixture,
}

impl BehavioralContext {
    pub fn new(label: &str, capabilities: &[&str]) -> Self {
        let workspace = support::TestWorkspace::new(label);
        let repository = support::TestWorkspace::new(&format!("{label}_repo"));
        let mut args = Vec::with_capacity(capabilities.len() * 2);
        for capability in capabilities {
            args.extend(["--capability", *capability]);
        }
        let server = support::HttpServer::start_with_args(
            workspace.path(),
            API_TOKEN,
            &args,
            &[],
            Duration::from_secs(10),
        );
        let mut fixture = ProbeFixture::deterministic(workspace.path(), repository.path());
        fixture.clock_seconds = omakure::enrollment::now_seconds();
        Self {
            workspace,
            repository,
            server,
            fixture,
        }
    }

    pub fn new_node(label: &str, capabilities: &[&str]) -> Self {
        let workspace = support::TestWorkspace::new(label);
        let repository = support::TestWorkspace::new(&format!("{label}_repo"));
        let state = workspace.path().join(".node-state");
        let config = workspace.path().join("node.toml");
        let state_arg = state.to_string_lossy().to_string();
        let config_arg = config.to_string_lossy().to_string();
        let mut init = support::omakure_command();
        init.args([
            "--scripts-dir",
            workspace.path().to_str().expect("workspace path is UTF-8"),
            "--json",
            "node",
            "--node-state-dir",
            &state_arg,
            "--node-config",
            &config_arg,
            "init",
        ])
        .env("OMAKURE_API_TOKEN", API_TOKEN)
        .env("OMAKURE_NODE_TEST_MODE", "1")
        .env("OMAKURE_NODE_STATE_DIR", &state_arg)
        .env("OMAKURE_NODE_CONFIG", &config_arg);
        let output = support::command_with_timeout(&mut init, Duration::from_secs(10));
        assert!(
            output.status.success(),
            "node fixture initialization failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut args = Vec::with_capacity(capabilities.len() * 2);
        for capability in capabilities {
            args.extend(["--capability", *capability]);
        }
        let server = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            support::HttpServer::start_node_service(
                workspace.path(),
                API_TOKEN,
                &args,
                &[],
                Duration::from_secs(10),
            )
        }))
        .unwrap_or_else(|_| panic!("node fixture {label} failed startup"));
        let mut fixture = ProbeFixture::deterministic(workspace.path(), repository.path());
        fixture.clock_seconds = omakure::enrollment::now_seconds();
        Self {
            workspace,
            repository,
            server,
            fixture,
        }
    }

    pub fn derive(&self, suffix: &str, capabilities: &[&str]) -> Self {
        let mut derived = Self::new(&format!("parity_{suffix}"), capabilities);
        derived.fixture.clock_seconds = self.fixture.clock_seconds;
        derived.fixture.generated_ids = self.fixture.generated_ids.clone();
        derived.fixture.authorized_actor = self.fixture.authorized_actor.clone();
        derived.fixture.unauthenticated_actor = self.fixture.unauthenticated_actor.clone();
        derived.fixture.forbidden_actor = self.fixture.forbidden_actor.clone();
        derived
    }

    pub fn derive_node(&self, suffix: &str, capabilities: &[&str]) -> Self {
        let mut derived = Self::new_node(&format!("parity_{suffix}"), capabilities);
        derived.fixture.clock_seconds = self.fixture.clock_seconds;
        derived.fixture.generated_ids = self.fixture.generated_ids.clone();
        derived.fixture.authorized_actor = self.fixture.authorized_actor.clone();
        derived.fixture.unauthenticated_actor = self.fixture.unauthenticated_actor.clone();
        derived.fixture.forbidden_actor = self.fixture.forbidden_actor.clone();
        derived
    }

    pub fn authorized_actor(&self) -> &str {
        &self.fixture.authorized_actor
    }

    pub fn unauthenticated_actor(&self) -> &str {
        &self.fixture.unauthenticated_actor
    }

    pub fn forbidden_actor(&self) -> &str {
        &self.fixture.forbidden_actor
    }

    pub fn clock_seconds(&self) -> u64 {
        self.fixture.clock_seconds
    }
    pub fn fresh_clock_seconds(&self) -> u64 {
        omakure::enrollment::now_seconds()
    }

    pub fn cli(&self, args: &[&str]) -> Output {
        self.cli_with_env(args, &[])
    }

    pub fn cli_with_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut command = support::omakure_command();
        command
            .arg("--scripts-dir")
            .arg(self.workspace.path())
            .args(args)
            .env("OMAKURE_API_TOKEN", API_TOKEN);
        for (key, value) in envs {
            command.env(key, value);
        }
        support::command_with_timeout(&mut command, Duration::from_secs(10))
    }

    pub fn cli_json(&self, args: &[&str]) -> Value {
        let mut json_args = Vec::with_capacity(args.len() + 1);
        if !args.contains(&"--json") {
            json_args.push("--json");
        }
        json_args.extend_from_slice(args);
        let output = self.cli(&json_args);
        assert!(output.status.success(), "CLI failed for {args:?}");
        support::json_envelope(&output.stdout)
    }

    pub fn http_json(&self, response: support::HttpResponse) -> (u16, Value) {
        (response.status, response.json())
    }

    pub fn http_get(&self, path: &str) -> support::HttpResponse {
        self.server.get(path)
    }

    pub fn http_get_unauthenticated(&self, path: &str) -> support::HttpResponse {
        self.server.get_unauthenticated(path)
    }

    pub fn http_get_forbidden(&self, path: &str) -> support::HttpResponse {
        self.server
            .get_with_bearer(path, "behavioral-parity-forbidden-token")
    }
}

pub fn evidence(cli: Value, http: (u16, Value)) -> Result<ProbeEvidence, String> {
    if http.0 >= 500 {
        return Err(format!("HTTP adapter failed with status {}", http.0));
    }
    Ok(ProbeEvidence {
        cli,
        http: http.1,
        semantic_difference: None,
    })
}

pub fn require_path(path: &Path) {
    assert!(
        path.exists(),
        "fixture path does not exist: {}",
        path.display()
    );
}
fn compact_evidence(value: &Value) -> String {
    let encoded = serde_json::to_string(value).unwrap_or_else(|_| "<non-json>".into());
    if encoded.chars().count() <= 512 {
        encoded
    } else {
        format!("{}…", encoded.chars().take(512).collect::<String>())
    }
}

type Probe = fn(&BehavioralContext) -> Result<ProbeEvidence, String>;

fn all_probes() -> Vec<(&'static str, Probe)> {
    core::probes()
        .into_iter()
        .chain(env_history_queue::probes())
        .chain(battery::probes())
        .chain(node::probes())
        .collect()
}

#[test]
fn every_manifest_case_executes_once_through_a_real_paired_probe() {
    let (manifest, cases) =
        omakure::cli_http_parity::checked_registry().expect("valid parity manifest");
    let mut expected: Vec<_> = cases
        .iter()
        .map(|case| case.behavior_case.as_str())
        .collect();
    expected.sort_unstable();

    let probes = all_probes();
    let mut actual: Vec<_> = probes.iter().map(|(case, _)| *case).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected, "probe registry must equal manifest cases");
    assert!(
        actual.windows(2).all(|pair| pair[0] != pair[1]),
        "duplicate probe case"
    );
    let family_count = cases
        .iter()
        .map(|case| case.operation_family.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    eprintln!(
        "CLI/HTTP behavioral parity: families={family_count} cases={} probes={}",
        cases.len(),
        probes.len()
    );

    let fixture = BehavioralContext::new("parity_dispatch", &["config:read"]);
    for (case_id, probe) in probes {
        let entry = cases
            .iter()
            .find(|case| case.behavior_case == case_id)
            .expect("case was checked against manifest");
        let evidence = probe(&fixture).unwrap_or_else(|error| panic!("{case_id}: {error}"));
        let schema = manifest
            .schemas
            .iter()
            .find(|schema| schema.operation_family == entry.operation_family)
            .expect("schema for behavior case");
        if entry.class == omakure::cli_http_parity::ParityClass::Exact {
            omakure::cli_http_parity::compare_observables_for_case(
                schema,
                case_id,
                &evidence.cli,
                &evidence.http,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "{case_id}: {error}; cli={}; http={}",
                    compact_evidence(&evidence.cli),
                    compact_evidence(&evidence.http)
                )
            });
            assert!(
                evidence.semantic_difference.is_none(),
                "{case_id} unexpectedly diverges"
            );
        } else {
            omakure::cli_http_parity::validate_observables_for_case(
                schema,
                case_id,
                &evidence.cli,
                &evidence.http,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "{case_id}: semantic evidence invalid: {error}; cli={}; http={}",
                    compact_evidence(&evidence.cli),
                    compact_evidence(&evidence.http)
                )
            });
            let expected_kind = manifest
                .entries
                .iter()
                .find(|candidate| candidate.behavior_case.as_deref() == Some(case_id))
                .and_then(|candidate| candidate.semantic_difference.as_ref())
                .map(|difference| difference.kind.as_str());
            assert_eq!(
                evidence.semantic_difference.as_deref(),
                expected_kind,
                "{case_id}"
            );
        }
    }
}

#[test]
fn derived_contexts_retain_fixture_actor_and_clock_values() {
    let mut context = BehavioralContext::new("parity_fixture_values", &["config:read"]);
    let clock_seconds = context.clock_seconds();
    context.fixture.authorized_actor = "fixture-authorized".into();
    context.fixture.unauthenticated_actor = "fixture-unauthenticated".into();
    context.fixture.forbidden_actor = "fixture-forbidden".into();

    let derived = context.derive("fixture_values_derived", &["config:read"]);
    assert_eq!(derived.authorized_actor(), "fixture-authorized");
    assert_eq!(derived.unauthenticated_actor(), "fixture-unauthenticated");
    assert_eq!(derived.forbidden_actor(), "fixture-forbidden");
    assert_eq!(derived.clock_seconds(), clock_seconds);

    let mut node_context = BehavioralContext::new_node("parity_fixture_values_node", &[]);
    let node_clock_seconds = node_context.clock_seconds();
    node_context.fixture.authorized_actor = "fixture-authorized".into();
    node_context.fixture.unauthenticated_actor = "fixture-unauthenticated".into();
    node_context.fixture.forbidden_actor = "fixture-forbidden".into();

    let derived_node = node_context.derive_node("fixture_values_node_derived", &[]);
    assert_eq!(derived_node.authorized_actor(), "fixture-authorized");
    assert_eq!(
        derived_node.unauthenticated_actor(),
        "fixture-unauthenticated"
    );
    assert_eq!(derived_node.forbidden_actor(), "fixture-forbidden");
    assert_eq!(derived_node.clock_seconds(), node_clock_seconds);
}

#[test]
fn fresh_clock_seconds_is_current_and_monotonic() {
    let context = BehavioralContext::new("parity_fresh_clock", &["config:read"]);
    let before = omakure::enrollment::now_seconds();
    let first = context.fresh_clock_seconds();
    let second = context.fresh_clock_seconds();
    let after = omakure::enrollment::now_seconds();

    assert!(
        before <= first && first <= second && second <= after,
        "fresh clock values must be current and monotonic: before={before}, first={first}, second={second}, after={after}"
    );
}

#[test]
fn behavioral_parity_harness_modules_compile() {
    let ids = [
        core::CASE_IDS,
        env_history_queue::CASE_IDS,
        battery::CASE_IDS,
        node::CASE_IDS,
    ]
    .into_iter()
    .flatten()
    .count();
    assert_eq!(ids, 44);
}
