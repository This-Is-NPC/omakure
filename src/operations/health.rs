//! Protocol-neutral Health Plane operations.
//!
//! Two responsibilities, both deliberately adapter-free:
//!
//! 1. [`fleet_status`] projects the Conductor-local Health Plane state that the
//!    Wave 2 shared operations own into one bounded, redacted report. The CLI
//!    and the HTTP route both render exactly this value, which is what makes
//!    them return identical status.
//! 2. [`signal_feed`] projects the bounded, newest-first closed Signal feed
//!    the same way, merging the per-Performer inbox with the Conductor-local
//!    lifecycle projection.
//! 3. [`NodeHealthFacts`] reads the live local node so a Performer can report
//!    Profile, Pulse, and `run-completed`. It reads the local node only;
//!    nothing here consults a peer message, and nothing here can mutate trust.
//!
//! Every bound below is transcribed from `.docs/health-plane-contract.md` via
//! `crate::health_plane::bounds`. None of them is chosen here.

use crate::health_plane::bounds::{
    MAX_PERFORMERS_PER_CONDUCTOR, RUNTIME_NAMES, SIGNAL_INBOX_CAPACITY, SIGNAL_RETENTION_SECONDS,
};
use crate::health_plane::model::{Presence, RunFact, RunnerFact, RuntimeFact, SignalRecord};
use crate::health_plane::report::{
    opaque_run_id, sanitize_signal_run, HealthFactsSource, ProfileFacts, PulseFacts,
};
use crate::health_plane::{FleetNode, HealthPlane};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerRole, PeerState};
use crate::operations::node::{map_identity_error, map_registry_error, registry_error};
use crate::operations::OperationResult;
use crate::runs::{self, RunState, RunStateSet};
use crate::workspace::Workspace;
use serde::Serialize;
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a runtime probe result is reused before the node re-probes.
///
/// Runtime detection spawns one short-lived process per runtime name. Caching
/// bounds that cost so a Profile-change check never becomes a fork storm, while
/// still noticing a newly installed interpreter within one window.
const RUNTIME_PROBE_TTL: Duration = Duration::from_secs(300);

/// Bytes read from `/etc/os-release`. The file is a few hundred bytes; the cap
/// makes a hostile or corrupt file a bounded read rather than an unbounded one.
const MAX_OS_RELEASE_BYTES: u64 = 64 * 1024;

/// Bytes of a `--version` banner kept before the version token is extracted.
const MAX_VERSION_BANNER_BYTES: usize = 256;

/// The bounded, redacted fleet-status projection.
///
/// This is current status only. It carries no chart series, no alert rule, no
/// arbitrary host inventory, no raw log, and no history: the Health Plane
/// stores exactly one Profile and one Pulse per Performer, and this report
/// shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FleetStatusReport {
    /// Whether Health Plane storage is available on this node.
    pub enabled: bool,
    /// The reporting node's own canonical node ID.
    pub local_node_id: String,
    /// The UTC Unix second the presence projection was derived at.
    pub observed_at: i64,
    /// Presence counts across every actively trusted peer.
    pub presence: PresenceCounts,
    /// One row per actively trusted peer, ordered by node ID.
    pub nodes: Vec<FleetNode>,
}

/// Presence totals derived from the frozen Pulse-age windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct PresenceCounts {
    pub online: usize,
    pub stale: usize,
    pub offline: usize,
    pub unknown: usize,
    pub total: usize,
}

/// The Conductor-local fleet-status projection.
///
/// The operation is read-only and derives presence through the Wave 2 shared
/// operations; it never reads a Health Plane table directly and never
/// re-implements authorization.
pub fn fleet_status(context: &NodeContext) -> OperationResult<FleetStatusReport> {
    let registry = open_registry(context)?;
    let plane = HealthPlane::new(&registry);
    let enabled = plane.enabled().map_err(map_registry_error)?;
    let observed_at = plane.now();
    let mut nodes = if enabled {
        plane.fleet_status().map_err(map_registry_error)?
    } else {
        Vec::new()
    };
    if enabled {
        // The fleet is the set of *actively trusted* peers. A peer whose trust
        // was revoked, suspended, or replaced is no longer part of it, so its
        // retained Health Plane row must not keep reporting a presence: that
        // is what makes a revocation change the operator's view immediately.
        // The decision uses the `trust_state` the Wave 2 projection already
        // computed from the local registry; nothing is re-derived here.
        nodes.retain(|node| node.trust_state == "active");

        // A peer that has never reported has no Health Plane row at all, so the
        // shared projection cannot see it. Enumerate the trusted peers and fill
        // in the never-seen ones, deciding trust *only* through the Wave 2
        // read-only authorization projection and deriving presence *only*
        // through the Wave 2 presence rule.
        let seen: HashSet<String> = nodes.iter().map(|node| node.node_id.clone()).collect();
        let candidates = registry
            .peers_limited(MAX_PERFORMERS_PER_CONDUCTOR as usize)
            .map_err(map_registry_error)?;
        for candidate in candidates {
            if seen.contains(&candidate.node_id) {
                continue;
            }
            let Some(authorization) = plane
                .authorization(&candidate.node_id)
                .map_err(map_registry_error)?
            else {
                continue;
            };
            if authorization.state != PeerState::Active {
                continue;
            }
            nodes.push(FleetNode {
                node_id: authorization.node_id,
                role: role_name(authorization.role).to_string(),
                capabilities: authorization.capabilities,
                trust_state: "active".to_string(),
                presence: Presence::derive(None, observed_at),
                last_pulse_at: None,
                profile: None,
                pulse: None,
                signal_cursor: 0,
                stored_signals: 0,
                held_signals: 0,
                version_incompatible: false,
            });
        }
        nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    }
    let mut presence = PresenceCounts {
        total: nodes.len(),
        ..PresenceCounts::default()
    };
    for node in &nodes {
        match node.presence {
            Presence::Online => presence.online += 1,
            Presence::Stale => presence.stale += 1,
            Presence::Offline => presence.offline += 1,
            Presence::Unknown => presence.unknown += 1,
        }
    }
    Ok(FleetStatusReport {
        enabled,
        local_node_id: registry.local_node_id().to_string(),
        observed_at,
        presence,
        nodes,
    })
}

/// One entry in the bounded, newest-first Signal feed.
///
/// Every field is privacy class P0. `source` is either the canonical node ID of
/// the Performer that reported the Signal over the wire, or `local` for the two
/// lifecycle kinds this Conductor decided itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalEntry {
    pub source: String,
    #[serde(flatten)]
    pub signal: SignalRecord,
}

/// The per-Performer cursor state the frozen ordering rules produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalCursor {
    pub node_id: String,
    /// The highest contiguously accepted Signal sequence for this Performer.
    pub cursor: u64,
    /// Signals the cursor has accepted and that are visible in the feed.
    pub stored: u64,
    /// Signals waiting in the bounded reorder buffer behind a gap.
    pub held: u64,
    /// Whether this Performer's feed is currently stalled behind a gap. The
    /// cursor never moves backwards and never skips, so a gap holds the feed
    /// rather than admitting a hole.
    pub gap: bool,
}

/// The bounded, newest-first Signal read surface.
///
/// This is a small closed feed, not history and not an event bus: exactly three
/// Signal kinds, one bounded page, one frozen retention window, no
/// subscriptions, no webhooks, and no filters that could turn it into a query
/// engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalFeedReport {
    /// Whether Health Plane storage is available on this node.
    pub enabled: bool,
    /// The reading node's own canonical node ID.
    pub local_node_id: String,
    /// The UTC Unix second the feed was derived at.
    pub observed_at: i64,
    /// The frozen Signal retention window, in seconds.
    pub retention_seconds: i64,
    /// The frozen bound on how many Signals one read may return.
    pub limit: usize,
    /// Whether any Performer feed is currently stalled behind a gap.
    pub gap: bool,
    /// Per-Performer cursor state, ordered by node ID.
    pub cursors: Vec<SignalCursor>,
    /// The bounded page, newest first.
    pub signals: Vec<SignalEntry>,
}

/// The Conductor-local Signal feed.
///
/// Read-only, bounded, and adapter-free: `omakure node signals --json` and
/// `GET /v1/node/signals` render exactly this value, which is what makes them
/// return identical results.
///
/// Both halves come from the Wave 2 shared operations. Remote Signals are the
/// bounded per-Performer inbox the ingest path filled; local `enrolled` and
/// `revoked` Signals are projected from the append-only trust audit, so they
/// survive the revocation cleanup that deletes every Health Plane row for a
/// peer that is no longer actively trusted.
pub fn signal_feed(context: &NodeContext) -> OperationResult<SignalFeedReport> {
    let registry = open_registry(context)?;
    let plane = HealthPlane::new(&registry);
    let enabled = plane.enabled().map_err(map_registry_error)?;
    let observed_at = plane.now();
    let limit = SIGNAL_INBOX_CAPACITY as usize;
    let mut entries: Vec<SignalEntry> = Vec::new();
    let mut cursors: Vec<SignalCursor> = Vec::new();
    if enabled {
        for signal in plane.local_signals(limit).map_err(map_registry_error)? {
            entries.push(SignalEntry {
                source: LOCAL_SIGNAL_SOURCE.to_string(),
                signal,
            });
        }
        let mut nodes = plane.fleet_status().map_err(map_registry_error)?;
        // The feed shows the *actively trusted* fleet, exactly like the
        // fleet-status projection: a peer whose trust was revoked, suspended,
        // or replaced stops appearing at once, which is what makes a
        // revocation change the operator's view immediately. The retained rows
        // are removed for good by the frozen revocation cleanup.
        nodes.retain(|node| node.trust_state == "active");
        nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        for node in nodes {
            cursors.push(SignalCursor {
                node_id: node.node_id.clone(),
                cursor: node.signal_cursor,
                stored: node.stored_signals,
                held: node.held_signals,
                gap: node.held_signals > 0,
            });
            for signal in plane
                .signals(&node.node_id, limit)
                .map_err(map_registry_error)?
            {
                entries.push(SignalEntry {
                    source: node.node_id.clone(),
                    signal,
                });
            }
            // Reduce after every peer rather than at the end, so the working
            // set stays at two pages regardless of how many Performers this
            // Conductor manages.
            reduce_to_newest(&mut entries, limit);
        }
    }
    reduce_to_newest(&mut entries, limit);
    Ok(SignalFeedReport {
        enabled,
        local_node_id: registry.local_node_id().to_string(),
        observed_at,
        retention_seconds: SIGNAL_RETENTION_SECONDS,
        limit,
        gap: cursors.iter().any(|cursor| cursor.gap),
        cursors,
        signals: entries,
    })
}

/// The `source` marker for a Signal this node decided itself.
const LOCAL_SIGNAL_SOURCE: &str = "local";

/// Order newest first and keep one bounded page.
///
/// The tiebreak on `signal_id` gives a total order, so two Signals that share
/// a second still render deterministically for both adapters.
fn reduce_to_newest(entries: &mut Vec<SignalEntry>, limit: usize) {
    entries.sort_by(|left, right| {
        right
            .signal
            .occurred_at
            .cmp(&left.signal.occurred_at)
            .then_with(|| right.signal.signal_id.cmp(&left.signal.signal_id))
    });
    entries.truncate(limit);
}

/// The stable wire name of a trusted peer role.
fn role_name(role: PeerRole) -> &'static str {
    match role {
        PeerRole::Conductor => "conductor",
        PeerRole::Performer => "performer",
    }
}

fn open_registry(context: &NodeContext) -> OperationResult<NodeRegistry> {
    let state_present = context
        .validate_existing_state_contents()
        .map_err(crate::operations::node::map_node_error)?;
    if !state_present {
        return Err(registry_error("node state is not initialized"));
    }
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    NodeRegistry::open_existing(context, identity.public_status()).map_err(map_registry_error)
}

/// The live local facts a Performer reports.
///
/// Sources are deliberately narrow: the shipped agent version, `std::env`
/// platform constants, `/etc/os-release`, the configured display name, the
/// four permitted runtime probes, and the run log. Nothing here reads a
/// hostname, a username, an address, a path, or a resource gauge, because none
/// of those is a privacy class P0 fact.
pub struct NodeHealthFacts {
    workspace: Workspace,
    display_name: String,
    workers_configured: u64,
    scheduler_enabled: bool,
    started: Instant,
    runtimes: Mutex<Option<(Instant, Vec<RuntimeFact>)>>,
}

impl NodeHealthFacts {
    /// Build a fact source for one running `node serve` process.
    pub(crate) fn new(
        workspace: Workspace,
        display_name: String,
        workers_configured: u64,
        scheduler_enabled: bool,
    ) -> Self {
        Self {
            workspace,
            display_name,
            workers_configured,
            scheduler_enabled,
            started: Instant::now(),
            runtimes: Mutex::new(None),
        }
    }

    fn cached_runtimes(&self) -> Vec<RuntimeFact> {
        let mut cache = self.runtimes.lock().expect("runtime probe cache");
        if let Some((probed_at, runtimes)) = cache.as_ref() {
            if probed_at.elapsed() < RUNTIME_PROBE_TTL {
                return runtimes.clone();
            }
        }
        let runtimes = probe_runtimes();
        *cache = Some((Instant::now(), runtimes.clone()));
        runtimes
    }
}

impl HealthFactsSource for NodeHealthFacts {
    fn profile_facts(&self) -> ProfileFacts {
        let os_release = read_os_release();
        let distro_id = os_release_value(&os_release, "ID");
        let distro_version = os_release_value(&os_release, "VERSION_ID");
        let is_omarchy = distro_id == "omarchy"
            || os_release_value(&os_release, "ID_LIKE")
                .split_whitespace()
                .any(|entry| entry == "omarchy");
        ProfileFacts {
            agent_version: crate::app_meta::APP_VERSION.to_string(),
            arch: match std::env::consts::ARCH {
                "x86_64" => "x86_64".to_string(),
                "aarch64" => "aarch64".to_string(),
                _ => "unknown".to_string(),
            },
            baseline_id: String::new(),
            baseline_observed_id: String::new(),
            capabilities: Vec::new(),
            display_name: self.display_name.clone(),
            distro_id,
            distro_version: distro_version.clone(),
            omarchy_channel: omarchy_channel(is_omarchy),
            omarchy_version: if is_omarchy {
                distro_version
            } else {
                String::new()
            },
            platform: match std::env::consts::OS {
                "macos" => "macos".to_string(),
                "windows" => "windows".to_string(),
                _ => "linux".to_string(),
            },
            runtimes: self.cached_runtimes(),
        }
    }

    fn pulse_facts(&self) -> PulseFacts {
        let uptime_seconds = self.started.elapsed().as_secs();
        let Ok(connection) = runs::open(&self.workspace) else {
            // The run log is unreadable. The contract has a state for exactly
            // this, and it is reported rather than guessed around.
            return PulseFacts {
                runner: RunnerFact {
                    queue_depth: 0,
                    scheduler: self.scheduler_state(),
                    state: "degraded".to_string(),
                    workers_busy: 0,
                    workers_configured: self.workers_configured,
                },
                last_run: None,
                uptime_seconds,
            };
        };
        let stats = runs::stats(&connection).ok();
        let count = |state: RunState| -> u64 {
            stats
                .as_ref()
                .and_then(|stats| stats.counts_by_state.get(state.as_str()).copied())
                .and_then(|value| u64::try_from(value).ok())
                .unwrap_or(0)
        };
        let queue_depth = count(RunState::Queued);
        let workers_busy = count(RunState::Running).min(self.workers_configured);
        let state = if self.workers_configured == 0 {
            "stopped"
        } else if workers_busy > 0 {
            "busy"
        } else {
            "idle"
        };
        PulseFacts {
            runner: RunnerFact {
                queue_depth,
                scheduler: self.scheduler_state(),
                state: state.to_string(),
                workers_busy,
                workers_configured: self.workers_configured,
            },
            last_run: last_terminal_run(&connection),
            uptime_seconds,
        }
    }

    fn terminal_runs(&self, limit: usize) -> Vec<RunFact> {
        let Ok(connection) = runs::open(&self.workspace) else {
            return Vec::new();
        };
        terminal_runs(&connection, limit.min(SIGNAL_INBOX_CAPACITY as usize))
    }
}

impl NodeHealthFacts {
    fn scheduler_state(&self) -> String {
        if self.scheduler_enabled {
            "running".to_string()
        } else {
            "disabled".to_string()
        }
    }
}

/// The most recent terminal run, mapped onto the frozen `last_run` shape.
///
/// Only the schema name, the opaque run id, the two timestamps, the state, the
/// trigger, and the exit code cross this boundary. The script path, the
/// arguments, stdout, stderr, the error text, the actor, and the worker id are
/// all privacy class P1 and never leave the run log.
fn last_terminal_run(connection: &rusqlite::Connection) -> Option<RunFact> {
    let filters = runs::RunFilters {
        limit: Some(1),
        states: RunStateSet::Terminal.to_states(),
        ..runs::RunFilters::default()
    };
    let row = runs::query_runs(connection, &filters)
        .ok()?
        .into_iter()
        .next()?;
    let finished_at = row.finished_at.map(|ms| ms / 1_000).filter(|at| *at >= 1)?;
    let started_at = row
        .started_at
        .map(|ms| ms / 1_000)
        .filter(|at| *at >= 1)
        .unwrap_or(finished_at);
    Some(RunFact {
        exit_code: row.exit_code.map(i64::from),
        finished_at,
        run_id: opaque_run_id(&row.run_id),
        script: run_script_name(row.script_name, &row.script_path),
        started_at: Some(started_at.min(finished_at)),
        state: row.state.as_str().to_string(),
        trigger: Some(match row.trigger {
            runs::RunTrigger::Scheduled => "scheduled".to_string(),
            runs::RunTrigger::Manual => "manual".to_string(),
            runs::RunTrigger::Cue => "cue".to_string(),
        }),
    })
}

/// The bounded, newest-first set of terminal runs, mapped onto the frozen
/// five-field Signal `run` object.
///
/// Only the schema name, the opaque run id, the finish time, the state, and
/// the exit code cross this boundary. The script path, the arguments, stdout,
/// stderr, the error text, the actor, and the worker id are privacy class P1
/// and never leave the run log. A row that cannot be expressed inside the
/// closed schema is dropped rather than guessed at.
fn terminal_runs(connection: &rusqlite::Connection, limit: usize) -> Vec<RunFact> {
    let filters = runs::RunFilters {
        limit: Some(limit as i64),
        states: RunStateSet::Terminal.to_states(),
        ..runs::RunFilters::default()
    };
    let Ok(rows) = runs::query_runs(connection, &filters) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|row| {
            let finished_at = row.finished_at.map(|ms| ms / 1_000).filter(|at| *at >= 1)?;
            let mut fact = RunFact {
                exit_code: row.exit_code.map(i64::from),
                finished_at,
                run_id: opaque_run_id(&row.run_id),
                script: run_script_name(row.script_name, &row.script_path),
                started_at: None,
                state: row.state.as_str().to_string(),
                trigger: None,
            };
            sanitize_signal_run(&mut fact).then_some(fact)
        })
        .collect()
}

/// The frozen `run.script` value for one run row.
///
/// The contract permits the script *schema name* and nothing else. The shipped
/// run log records `script_name` only for scheduler-enqueued runs
/// (`src/cli/serve.rs`); a manual `omakure run`, a queue enqueue, and
/// `POST /v1/runs` all record `None`, which would leave the frozen field empty
/// and make the whole run unrepresentable. The file stem - the very token
/// `omakure init` derives a script's canonical id from - is the fallback. A stem is the script's name, not its location: the
/// directory and the extension are dropped here, and the frozen grammar admits
/// no `/`, `\`, `:`, or `@`, so no path fragment can survive into a message.
fn run_script_name(script_name: Option<String>, script_path: &str) -> String {
    if let Some(name) = script_name.filter(|name| !name.trim().is_empty()) {
        return name;
    }
    // Both separators are handled explicitly rather than through `Path`, so a
    // Windows-shaped path recorded on a Linux node still yields a name and not
    // a path fragment.
    let base = script_path.rsplit(['/', '\\']).next().unwrap_or_default();
    base.rsplit_once('.')
        .map(|(stem, _)| stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or(base)
        .to_string()
}

fn read_os_release() -> String {
    for path in ["/etc/os-release", "/usr/lib/os-release"] {
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        if metadata.len() > MAX_OS_RELEASE_BYTES {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
    }
    String::new()
}

fn os_release_value(text: &str, key: &str) -> String {
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        return value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
    }
    String::new()
}

/// `stable` or `dev` for an Omarchy host, empty everywhere else.
///
/// `omarchy-version` treats an `OMARCHY_PATH` pointing away from the packaged
/// tree as a development checkout, so the same rule is applied here without
/// spawning it.
fn omarchy_channel(is_omarchy: bool) -> String {
    if !is_omarchy {
        return String::new();
    }
    match std::env::var("OMARCHY_PATH") {
        Ok(path) if path.trim_end_matches('/') != "/usr/share/omarchy" => "dev".to_string(),
        _ => "stable".to_string(),
    }
}

/// Probe the four permitted runtime names, in the frozen sorted order.
fn probe_runtimes() -> Vec<RuntimeFact> {
    RUNTIME_NAMES
        .iter()
        .map(|name| {
            let (program, args): (&str, &[&str]) = match *name {
                "bash" => ("bash", &["--version"]),
                "sh" => ("sh", &["-c", "echo ${BASH_VERSION:-}"]),
                "python" => (crate::runtime::python_program(), &["--version"]),
                _ => (
                    crate::runtime::powershell_program(),
                    &[
                        "-NoProfile",
                        "-Command",
                        "$PSVersionTable.PSVersion.ToString()",
                    ],
                ),
            };
            match probe_version(program, args) {
                Some(version) => RuntimeFact {
                    available: true,
                    name: (*name).to_string(),
                    version,
                },
                None => RuntimeFact {
                    available: false,
                    name: (*name).to_string(),
                    version: String::new(),
                },
            }
        })
        .collect()
}

fn probe_version(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let banner = String::from_utf8_lossy(&output.stdout);
    let banner = &banner[..banner.len().min(MAX_VERSION_BANNER_BYTES)];
    Some(version_token(banner))
}

/// Extract the first dotted version token from a `--version` banner.
///
/// Returns an empty string when no token is found, which the closed schema
/// accepts for an available runtime.
fn version_token(banner: &str) -> String {
    banner
        .split(|byte: char| byte.is_whitespace() || byte == '(' || byte == ')' || byte == ',')
        .find(|token| token.starts_with(|byte: char| byte.is_ascii_digit()) && token.contains('.'))
        .map(|token| {
            token
                .trim_end_matches(|byte: char| !byte.is_ascii_alphanumeric())
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_values_are_unquoted_and_key_exact() {
        let text = "NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\nVERSION_ID=\"4.0.1\"\n";
        assert_eq!(os_release_value(text, "ID"), "omarchy");
        assert_eq!(os_release_value(text, "VERSION_ID"), "4.0.1");
        assert_eq!(os_release_value(text, "ID_LIKE"), "arch");
        assert_eq!(os_release_value(text, "VERSION"), "");
    }

    #[test]
    fn a_version_token_is_extracted_from_a_banner_and_never_a_path() {
        assert_eq!(
            version_token("GNU bash, version 5.2.37(1)-release (x86_64-pc-linux-gnu)"),
            "5.2.37"
        );
        assert_eq!(version_token("Python 3.13.1"), "3.13.1");
        assert_eq!(version_token("7.4.6"), "7.4.6");
        assert_eq!(version_token("no version here"), "");
        assert_eq!(version_token("/usr/bin/bash"), "");
    }

    #[test]
    fn the_omarchy_channel_is_empty_off_omarchy_and_named_on_it() {
        assert_eq!(omarchy_channel(false), "");
        assert!(["stable", "dev"].contains(&omarchy_channel(true).as_str()));
    }

    #[test]
    fn a_run_script_name_is_a_name_and_never_a_path() {
        assert_eq!(
            run_script_name(Some("deploy".to_string()), "/srv/scripts/other.sh"),
            "deploy",
            "an explicit schema name always wins"
        );
        assert_eq!(
            run_script_name(None, "/home/operator/workspace/tools/deploy.sh"),
            "deploy"
        );
        assert_eq!(
            run_script_name(Some(String::new()), "tools/backup.ps1"),
            "backup"
        );
        assert_eq!(run_script_name(None, ""), "");
        for derived in [
            run_script_name(None, "/home/operator/workspace/tools/deploy.sh"),
            run_script_name(None, "C:\\Users\\op\\tools\\deploy.ps1"),
        ] {
            for forbidden in ['/', '\\', ':', '@'] {
                assert!(
                    !derived.contains(forbidden),
                    "{derived} carried a path separator"
                );
            }
        }
    }

    #[test]
    fn presence_counts_default_to_zero() {
        let counts = PresenceCounts::default();
        assert_eq!(counts.total, 0);
        assert_eq!(
            counts.online + counts.stale + counts.offline + counts.unknown,
            0
        );
    }
}
