use super::payload::{profile_payload, pulse_payload};
use super::sanitize::{
    sanitize_capabilities, sanitize_profile, sanitize_pulse, sanitize_signal_run,
};
use super::{HealthFactsSource, ProfileFacts, ProfileMessage, PulseMessage};
use crate::health_plane::bounds::{NOMINAL_PULSE_INTERVAL_SECONDS, SIGNAL_OUTBOX_CAPACITY};
use crate::health_plane::model::RunFact;
use std::sync::Mutex;

#[derive(Debug, Default)]
struct ReporterState {
    current: Option<ProfileFacts>,
    profile_revision: u64,
    last_pulse_sequence: u64,
    /// The newest `finished_at` this reporter has already turned into a
    /// `run-completed` Signal. `None` until the first harvest seeds it.
    run_watermark: Option<i64>,
    /// The opaque run ids already harvested at exactly `run_watermark`, so two
    /// runs that finish inside the same Unix second both produce a Signal and
    /// neither produces two. Bounded by the frozen outbox capacity.
    run_watermark_ids: Vec<String>,
}

/// Builds the Profile and Pulse payloads one Performer sends to its Conductor.
///
/// Revision and sequence are wall-clock derived rather than stored, which is
/// what makes them survive a restart without a second source of truth:
///
/// * `profile_revision` is the Unix second at which the current facts were
///   first observed, floored to strictly exceed the previous revision. A
///   restart therefore always produces a revision greater than the one a
///   Conductor already holds.
/// * `pulse.sequence` is the emitting Unix second, which the contract also
///   requires `emitted_at` to equal. The frozen 10-second minimum accepted
///   Pulse interval guarantees two accepted Pulses never share a second, so
///   the sequence is strictly increasing per sender across restarts.
pub struct HealthReporter {
    facts: Box<dyn HealthFactsSource>,
    state: Mutex<ReporterState>,
}

impl HealthReporter {
    /// Build a reporter over one live fact source.
    pub fn new(facts: Box<dyn HealthFactsSource>) -> Self {
        Self {
            facts,
            state: Mutex::new(ReporterState::default()),
        }
    }

    /// The frozen nominal interval between Pulses, in seconds.
    pub const fn pulse_interval_seconds() -> i64 {
        NOMINAL_PULSE_INTERVAL_SECONDS
    }

    /// Build the current Profile for one Conductor.
    ///
    /// `granted` is the capability set the local registry records for that
    /// Conductor. It is display-only: the receiver authorizes from its own
    /// registry and never from this field.
    pub fn profile(
        &self,
        target: &str,
        message_id: &str,
        granted: &[String],
        now: i64,
    ) -> ProfileMessage {
        let mut facts = self.facts.profile_facts();
        facts.capabilities = sanitize_capabilities(granted);
        sanitize_profile(&mut facts);
        let mut state = self.state.lock().expect("health reporter state");
        let changed = state.current.as_ref() != Some(&facts);
        if changed || state.profile_revision == 0 {
            let floor = state.profile_revision.saturating_add(1);
            state.profile_revision = u64::try_from(now).unwrap_or(1).max(floor).max(1);
            state.current = Some(facts.clone());
        }
        let profile_revision = state.profile_revision;
        drop(state);
        ProfileMessage {
            payload: profile_payload(target, message_id, &facts, profile_revision),
            profile_revision,
            changed,
        }
    }

    /// Whether the live facts differ from the last Profile this reporter built.
    pub fn profile_changed(&self, granted: &[String]) -> bool {
        let mut facts = self.facts.profile_facts();
        facts.capabilities = sanitize_capabilities(granted);
        sanitize_profile(&mut facts);
        let state = self.state.lock().expect("health reporter state");
        state.current.as_ref() != Some(&facts)
    }

    /// Build the current Pulse for one Conductor.
    ///
    /// Returns `None` when `now` would not produce a strictly increasing
    /// sequence, which is exactly the case the frozen minimum Pulse interval
    /// already forbids on the wire.
    pub fn pulse(&self, target: &str, message_id: &str, now: i64) -> Option<PulseMessage> {
        let sequence = u64::try_from(now).ok()?;
        let mut state = self.state.lock().expect("health reporter state");
        if sequence <= state.last_pulse_sequence {
            return None;
        }
        state.last_pulse_sequence = sequence;
        let profile_revision = state.profile_revision;
        drop(state);
        let mut facts = self.facts.pulse_facts();
        sanitize_pulse(&mut facts);
        Some(PulseMessage {
            payload: pulse_payload(target, message_id, &facts, profile_revision, sequence),
            sequence,
        })
    }

    /// Harvest the terminal runs that still need a `run-completed` Signal.
    ///
    /// The frozen contract emits this Signal *only after* the existing run
    /// state reaches a terminal result, so the run log is the sole trigger and
    /// nothing here starts, schedules, or observes live work.
    ///
    /// The first call seeds the watermark from whatever the run log already
    /// holds and returns nothing: a Performer that restarts must not replay its
    /// own history into a Conductor's bounded Signal inbox. Every later call
    /// returns only the runs that reached a terminal result after that point,
    /// oldest first, bounded by the frozen outbox capacity.
    pub fn run_signals(&self) -> Vec<RunFact> {
        let capacity = SIGNAL_OUTBOX_CAPACITY as usize;
        let runs = self.sanitized_terminal_runs(capacity);
        let mut state = self.state.lock().expect("health reporter state");
        let Some(watermark) = state.run_watermark else {
            seed_watermark(&mut state, &runs, capacity);
            return Vec::new();
        };
        let mut emitted = Vec::new();
        for run in runs {
            if run.finished_at < watermark {
                continue;
            }
            if run.finished_at == watermark && state.run_watermark_ids.contains(&run.run_id) {
                continue;
            }
            if run.finished_at > state.run_watermark.unwrap_or(watermark) {
                state.run_watermark = Some(run.finished_at);
                state.run_watermark_ids.clear();
            }
            state.run_watermark_ids.push(run.run_id.clone());
            if state.run_watermark_ids.len() > capacity {
                state.run_watermark_ids.remove(0);
            }
            emitted.push(run);
        }
        emitted
    }

    /// Seed the run watermark now, without consuming anything.
    ///
    /// The first `run_signals` call seeds and returns nothing, so a run that
    /// reaches a terminal result before that call is swallowed forever. That is
    /// correct for history a restarting Performer must not replay, and wrong
    /// for a run some *other* node asked for and is waiting on. Seeding at
    /// service start, before the transport can accept anything, makes the two
    /// cases distinguishable by time rather than by luck.
    ///
    /// Idempotent under the same lock that guards the watermark, so a second
    /// caller cannot turn this into a silent harvest.
    pub fn seed_run_watermark(&self) {
        let capacity = SIGNAL_OUTBOX_CAPACITY as usize;
        let runs = self.sanitized_terminal_runs(capacity);
        let mut state = self.state.lock().expect("health reporter state");
        if state.run_watermark.is_some() {
            return;
        }
        seed_watermark(&mut state, &runs, capacity);
    }

    /// The terminal runs a Signal may describe, sanitized and oldest first.
    fn sanitized_terminal_runs(&self, capacity: usize) -> Vec<RunFact> {
        let mut runs: Vec<RunFact> = self
            .facts
            .terminal_runs(capacity)
            .into_iter()
            .filter_map(|mut run| sanitize_signal_run(&mut run).then_some(run))
            .collect();
        runs.sort_by(|left, right| {
            left.finished_at
                .cmp(&right.finished_at)
                .then_with(|| left.run_id.cmp(&right.run_id))
        });
        runs.truncate(capacity);
        runs
    }
}

/// Record the newest terminal result as already reported.
fn seed_watermark(state: &mut ReporterState, runs: &[RunFact], capacity: usize) {
    let newest = runs.last().map(|run| run.finished_at).unwrap_or(0);
    state.run_watermark = Some(newest);
    state.run_watermark_ids = runs
        .iter()
        .filter(|run| run.finished_at == newest)
        .map(|run| run.run_id.clone())
        .collect();
    state.run_watermark_ids.truncate(capacity);
}
