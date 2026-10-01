use crate::health_plane::bounds::{
    NOMINAL_PULSE_INTERVAL_SECONDS, RUNTIME_NAMES, SIGNAL_INBOX_CAPACITY,
};
use crate::health_plane::model::{RunFact, RunnerFact, RuntimeFact};
use crate::health_plane::report::{
    HealthFactsSource, ProfileFacts, PulseFacts, opaque_run_id, sanitize_signal_run,
};
use crate::runs::{self, RunState, RunStateSet, RunStore};
use crate::workspace::Workspace;
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
        if let Some((_, facts)) = cache
            .as_ref()
            .filter(|(observed_at, _)| observed_at.elapsed() < BASELINE_OBSERVE_TTL)
        {
            return facts.clone();
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
        if let Some((_, runtimes)) = cache
            .as_ref()
            .filter(|(probed_at, _)| probed_at.elapsed() < RUNTIME_PROBE_TTL)
        {
            return runtimes.clone();
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
        let Ok(store) = RunStore::open(&self.workspace) else {
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
        let stats = store.stats().ok();
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
            last_run: last_terminal_run(&store),
            uptime_seconds,
        }
    }

    fn terminal_runs(&self, limit: usize) -> Vec<RunFact> {
        let Ok(store) = RunStore::open(&self.workspace) else {
            return Vec::new();
        };
        terminal_runs(&store, limit.min(SIGNAL_INBOX_CAPACITY as usize))
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
fn last_terminal_run(store: &RunStore) -> Option<RunFact> {
    let filters = runs::RunFilters {
        limit: Some(1),
        states: RunStateSet::Terminal.to_states(),
        ..runs::RunFilters::default()
    };
    let row = store.query_runs(&filters).ok()?.into_iter().next()?;
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
fn terminal_runs(store: &RunStore, limit: usize) -> Vec<RunFact> {
    let filters = runs::RunFilters {
        limit: Some(limit as i64),
        states: RunStateSet::Terminal.to_states(),
        ..runs::RunFilters::default()
    };
    let Ok(rows) = store.query_runs(&filters) else {
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
/// (`src/cli/serve/`); a manual `omakure run`, a queue enqueue, and
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
/// has to read frames off the transport socket. A child that never exits would park the link
/// for as long as it stayed wedged. An interpreter that cannot say its version
/// in five seconds is reported unavailable, which is the same answer as one
/// that is not installed and is the honest one either way.
const RUNTIME_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

fn probe_version(program: &str, args: &[&str]) -> Option<String> {
    crate::adapters::system_checks::runtime_version_banner(
        program,
        args,
        RUNTIME_PROBE_TIMEOUT,
        MAX_VERSION_BANNER_BYTES,
    )
    .map(|banner| version_token(&banner))
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
    use crate::test_support::workspace_in;

    #[test]
    fn an_empty_run_store_reports_idle_and_no_terminal_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let facts = NodeHealthFacts::new(workspace_in(&dir), "node".to_string(), 2, true);

        let pulse = facts.pulse_facts();
        assert_eq!(pulse.runner.state, "idle");
        assert_eq!(pulse.runner.scheduler, "running");
        assert_eq!(pulse.runner.queue_depth, 0);
        assert_eq!(pulse.runner.workers_busy, 0);
        assert_eq!(pulse.runner.workers_configured, 2);
        assert!(pulse.last_run.is_none());
        assert!(facts.terminal_runs(1).is_empty());
    }

    #[test]
    fn an_unreadable_run_store_degrades_pulse_and_omits_terminal_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = workspace_in(&dir);
        std::fs::remove_dir_all(workspace.history_dir()).expect("remove history directory");
        std::fs::write(workspace.history_dir(), b"blocked").expect("block history directory");
        let facts = NodeHealthFacts::new(workspace, "node".to_string(), 2, false);

        let pulse = facts.pulse_facts();
        assert_eq!(pulse.runner.state, "degraded");
        assert_eq!(pulse.runner.scheduler, "disabled");
        assert_eq!(pulse.runner.queue_depth, 0);
        assert_eq!(pulse.runner.workers_busy, 0);
        assert_eq!(pulse.runner.workers_configured, 2);
        assert!(pulse.last_run.is_none());
        assert!(facts.terminal_runs(1).is_empty());
    }

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
        let open = || workspace_in(&dir);
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
}
