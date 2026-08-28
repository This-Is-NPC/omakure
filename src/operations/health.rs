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
//! Every bound below is transcribed from `docs/internal/health-plane-contract.md` via
//! `crate::health_plane::bounds`. None of them is chosen here.

use crate::health_plane::bounds::{
    MAX_PERFORMERS_PER_CONDUCTOR, NOMINAL_PULSE_INTERVAL_SECONDS, RUNTIME_NAMES,
    SIGNAL_INBOX_CAPACITY, SIGNAL_RETENTION_SECONDS,
};
use crate::health_plane::model::{Presence, RunFact, RunnerFact, RuntimeFact, SignalRecord};
use crate::health_plane::report::{
    opaque_run_id, sanitize_signal_run, HealthFactsSource, ProfileFacts, PulseFacts,
};
use crate::health_plane::{BaselineStatus, FleetNode, HealthPlane};
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

/// How long one recomputed baseline observation is reused.
///
/// The same shape and the same reason as [`RUNTIME_PROBE_TTL`]: the
/// Profile-change check runs on a one-second tick, and re-hashing every script
/// a baseline names that often would turn a fleet-wide drift answer into
/// continuous disk work. Bounded to the frozen nominal Pulse interval rather
/// than to a number chosen here, because a fact cannot usefully change faster
/// than this node reports anything — which also makes it the worst-case delay
/// between a script changing underneath a Performer and its Conductor seeing
/// the drift.
const BASELINE_OBSERVE_TTL: Duration = Duration::from_secs(NOMINAL_PULSE_INTERVAL_SECONDS as u64);

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
    /// Baseline verdicts across the same peers, so "which machines drifted" is
    /// one read rather than a scan of every row.
    pub baselines: BaselineCounts,
    /// One row per actively trusted peer, ordered by node ID.
    pub nodes: Vec<FleetNode>,
}

/// Baseline verdict totals, derived from the same stored Profiles the rows show.
///
/// `unknown` and `none` are separate totals on purpose: a fleet with ten
/// machines that have never reported and a fleet with ten that hold no baseline
/// are different situations, and one number covering both would tell an
/// operator to go looking in the wrong place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct BaselineCounts {
    pub in_sync: usize,
    pub drifted: usize,
    pub none: usize,
    pub unknown: usize,
    pub total: usize,
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
                baseline_status: BaselineStatus::Unknown,
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
    let mut baselines = BaselineCounts {
        total: nodes.len(),
        ..BaselineCounts::default()
    };
    for node in &nodes {
        match node.presence {
            Presence::Online => presence.online += 1,
            Presence::Stale => presence.stale += 1,
            Presence::Offline => presence.offline += 1,
            Presence::Unknown => presence.unknown += 1,
        }
        match node.baseline_status {
            BaselineStatus::InSync => baselines.in_sync += 1,
            BaselineStatus::Drifted => baselines.drifted += 1,
            BaselineStatus::None => baselines.none += 1,
            BaselineStatus::Unknown => baselines.unknown += 1,
        }
    }
    Ok(FleetStatusReport {
        enabled,
        local_node_id: registry.local_node_id().to_string(),
        observed_at,
        presence,
        baselines,
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
    let limit = SIGNAL_INBOX_CAPACITY as usize;
    let mut observed_at = plane.now();
    let mut entries: Vec<SignalEntry> = Vec::new();
    let mut cursors: Vec<SignalCursor> = Vec::new();
    if enabled {
        // One snapshot for the cursors, the Signals, and the trust log the
        // local lifecycle Signals are projected from. Read separately, the
        // report could contradict itself: ingest commits between the counter
        // read and the Signal read, and the feed then shows a Signal beside a
        // cursor that has not counted it. `gap` is derived from those same
        // counters, and it is the field an operator reads to decide whether a
        // fleet's Signal delivery has stalled.
        let feed = plane.signal_feed(limit).map_err(map_registry_error)?;
        observed_at = feed.observed_at;
        for signal in feed.local {
            entries.push(SignalEntry {
                source: LOCAL_SIGNAL_SOURCE.to_string(),
                signal,
            });
        }
        // The feed shows the *actively trusted* fleet, exactly like the
        // fleet-status projection: a peer whose trust was revoked, suspended,
        // or replaced stops appearing at once, which is what makes a
        // revocation change the operator's view immediately. The retained rows
        // are removed for good by the frozen revocation cleanup.
        let mut active: HashSet<String> = HashSet::new();
        for node in feed.nodes {
            if node.trust_state != "active" {
                continue;
            }
            active.insert(node.node_id.clone());
            cursors.push(SignalCursor {
                node_id: node.node_id,
                cursor: node.cursor,
                stored: node.stored,
                held: node.held,
                gap: node.held > 0,
            });
        }
        cursors.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        for entry in feed.signals {
            // The bounded page is already restricted to actively trusted
            // peers by the read itself; this repeats the decision here so the
            // rule stays visible where the projection is assembled.
            if !active.contains(&entry.source) {
                continue;
            }
            entries.push(SignalEntry {
                source: entry.source,
                signal: entry.signal,
            });
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
    baseline: Mutex<Option<(Instant, BaselineFacts)>>,
}

/// The claim and the evidence, as one Performer can currently answer them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BaselineFacts {
    recorded: String,
    observed: String,
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
            baseline: Mutex::new(None),
        }
    }

    /// The baseline this node recorded installing, beside the one it can
    /// currently see.
    ///
    /// Reading the record and re-hashing the set are one cached step because
    /// they must describe the same instant: a claim read before an install and
    /// evidence gathered after it would report a machine as drifted at the one
    /// moment it is certainly not.
    fn cached_baseline(&self) -> BaselineFacts {
        let mut cache = self.baseline.lock().expect("baseline observation cache");
        if let Some((observed_at, facts)) = cache.as_ref() {
            if observed_at.elapsed() < BASELINE_OBSERVE_TTL {
                return facts.clone();
            }
        }
        let facts = match crate::operations::baseline::installed_baseline(&self.workspace) {
            Some(record) => BaselineFacts {
                observed: crate::operations::baseline::observed_baseline_id(
                    &self.workspace,
                    &record,
                ),
                recorded: record.baseline_id,
            },
            None => BaselineFacts::default(),
        };
        *cache = Some((Instant::now(), facts.clone()));
        facts
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
        let baseline = self.cached_baseline();
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
            baseline_id: baseline.recorded,
            baseline_observed_id: baseline.observed,
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

/// How long one interpreter gets to answer `--version`.
///
/// A Profile is built on the Performer's own session loop, the loop that also
/// has to read frames off the transport socket. `Command::output()` waits for
/// as long as the child takes, so one wedged interpreter would park the link
/// for as long as it stayed wedged. An interpreter that cannot say its version
/// in five seconds is reported unavailable, which is the same answer as one
/// that is not installed and is the honest one either way.
const RUNTIME_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

fn probe_version(program: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + RUNTIME_PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => return None,
        }
    };
    if !status.success() {
        return None;
    }

    let mut banner = String::new();
    child
        .stdout
        .take()?
        .take(MAX_VERSION_BANNER_BYTES as u64)
        .read_to_string(&mut banner)
        .ok()?;
    Some(version_token(&banner))
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

    /// A wedged interpreter must not hold the Profile open.
    ///
    /// The probe runs on the Performer's session loop, so an unbounded wait
    /// here is an unbounded wait on the transport. Measured against `sleep`,
    /// which is the cheapest program that reliably does nothing for longer
    /// than the budget.
    #[test]
    fn a_runtime_that_will_not_answer_is_given_up_on() {
        let started = Instant::now();
        let probed = probe_version("sleep", &["60"]);
        let waited = started.elapsed();

        assert!(
            probed.is_none(),
            "a program that never prints a version must be reported unavailable"
        );
        assert!(
            waited < RUNTIME_PROBE_TIMEOUT * 2,
            "the probe waited {waited:?}, which is past its own budget of {RUNTIME_PROBE_TIMEOUT:?}"
        );
    }

    /// The bound must not cost the answer in the ordinary case.
    #[test]
    fn a_runtime_that_answers_is_still_read() {
        let probed = probe_version("sh", &["-c", "echo 1.2.3"]);
        assert_eq!(probed.as_deref(), Some("1.2.3"));
    }

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

    /// The pair a Conductor compares comes off this node's own disk.
    ///
    /// The two halves are built in different modules — the record is written by
    /// the install path and the observation is recomputed by the drift path —
    /// and this is the only place they meet. A node with no baseline reports an
    /// empty pair rather than an invented one, which is what makes "never
    /// pushed" a different answer from "in sync".
    #[test]
    fn a_performer_reports_the_baseline_it_holds_and_the_one_on_its_disk() {
        use k256::schnorr::SigningKey;
        use sha2::{Digest, Sha256};

        let dir = tempfile::tempdir().expect("tempdir");
        let open = || {
            let workspace = crate::workspace::Workspace::new(dir.path().to_path_buf());
            workspace.ensure_layout().expect("layout");
            workspace
        };
        // The observation is cached to a bounded window, so each stage builds a
        // fresh fact source: this test is about what a Performer reports, not
        // about how long it reuses an answer.
        let facts = NodeHealthFacts::new(open(), "certification".to_string(), 1, false);

        let empty = facts.profile_facts();
        assert_eq!(
            (
                empty.baseline_id.as_str(),
                empty.baseline_observed_id.as_str()
            ),
            ("", ""),
            "a node that was never pushed a baseline has neither a claim nor evidence"
        );

        let bodies = vec![("ops/deploy.sh".to_string(), b"echo deploy\n".to_vec())];
        let signing_key = SigningKey::from_slice(&[7u8; 32]).expect("scalar");
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
        let mut key_id = [0u8; 16];
        key_id.copy_from_slice(&Sha256::digest(public_key)[..16]);
        let baseline = crate::baseline::SignedBaselineManifest::sign_with_material(
            signing_key.to_bytes().as_ref(),
            key_id,
            "acme".to_string(),
            &bodies,
            1_800_000_000,
            1_800_003_600,
        )
        .expect("sign")
        .bind(bodies)
        .expect("bind");
        let record =
            crate::operations::baseline::install_baseline(&open(), &baseline, 1_800_000_100)
                .expect("install");

        let facts = NodeHealthFacts::new(open(), "certification".to_string(), 1, false);
        let installed = facts.profile_facts();
        assert_eq!(installed.baseline_id, record.baseline_id);
        assert_eq!(
            installed.baseline_observed_id, record.baseline_id,
            "a node running what it was pushed reports one identity twice"
        );

        std::fs::write(
            open().scripts_root().join("ops/deploy.sh"),
            b"echo deploy\necho edited\n",
        )
        .expect("edit");
        let facts = NodeHealthFacts::new(open(), "certification".to_string(), 1, false);
        let drifted = facts.profile_facts();
        assert_eq!(
            drifted.baseline_id, record.baseline_id,
            "editing a script does not change what this node was given"
        );
        assert_ne!(
            drifted.baseline_observed_id, record.baseline_id,
            "editing a script does change what this node is holding"
        );
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
