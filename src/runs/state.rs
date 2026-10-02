use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// RunState enum
// ---------------------------------------------------------------------------

/// Final, closed set of legal values for the `runs.state` column.
///
/// Adding a new variant is a deliberate breaking change to the AI contract.
/// The state set is intentionally small and has no `paused`, `retrying`,
/// `scheduled`, `expired`, `zombie`, or `blocked` member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    DeadLetter,
}

impl RunState {
    /// Stable string representation written into the `state` column and
    /// returned in JSON envelopes. Renaming any of these strings is a
    /// breaking change.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Queued => "queued",
            RunState::Running => "running",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
            RunState::TimedOut => "timed_out",
            RunState::DeadLetter => "dead_letter",
        }
    }

    /// All seven legal values, in stable order.
    pub fn all() -> &'static [RunState] {
        &[
            RunState::Queued,
            RunState::Running,
            RunState::Completed,
            RunState::Failed,
            RunState::Cancelled,
            RunState::TimedOut,
            RunState::DeadLetter,
        ]
    }
}

impl fmt::Display for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RunState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(RunState::Queued),
            "running" => Ok(RunState::Running),
            "completed" => Ok(RunState::Completed),
            "failed" => Ok(RunState::Failed),
            "cancelled" => Ok(RunState::Cancelled),
            "timed_out" => Ok(RunState::TimedOut),
            "dead_letter" => Ok(RunState::DeadLetter),
            other => Err(format!(
                "invalid run state '{}': expected one of queued, running, completed, failed, cancelled, timed_out, dead_letter",
                other
            )),
        }
    }
}

impl Serialize for RunState {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunState {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Shorthand for groups of states used by `--state-set` on `history list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStateSet {
    InFlight,
    Terminal,
    All,
}

impl RunStateSet {
    pub fn to_states(self) -> Vec<RunState> {
        match self {
            RunStateSet::InFlight => vec![RunState::Queued, RunState::Running],
            RunStateSet::Terminal => vec![
                RunState::Completed,
                RunState::Failed,
                RunState::Cancelled,
                RunState::TimedOut,
                RunState::DeadLetter,
            ],
            RunStateSet::All => RunState::all().to_vec(),
        }
    }
}

impl FromStr for RunStateSet {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "in_flight" => Ok(RunStateSet::InFlight),
            "terminal" => Ok(RunStateSet::Terminal),
            "all" => Ok(RunStateSet::All),
            other => Err(format!(
                "invalid state-set '{}': expected one of in_flight, terminal, all",
                other
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// RunTrigger
// ---------------------------------------------------------------------------

/// Provenance of a run row: did a human launch it, did the scheduler, or did an
/// authorized Conductor?
///
/// `Cue` is not cosmetic. It is the discriminator that keeps a remotely
/// initiated run out of the lease-steal path, and without it the Health Plane
/// reports such a run as `manual` — a false audit record in exactly the feature
/// whose purpose is distributed audit outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RunTrigger {
    #[default]
    Manual,
    Scheduled,
    Cue,
    Workflow,
}

impl RunTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunTrigger::Manual => "Manual",
            RunTrigger::Scheduled => "Scheduled",
            RunTrigger::Cue => "Cue",
            RunTrigger::Workflow => "Workflow",
        }
    }
}

impl fmt::Display for RunTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RunTrigger {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Manual" => Ok(RunTrigger::Manual),
            "Scheduled" => Ok(RunTrigger::Scheduled),
            "Cue" => Ok(RunTrigger::Cue),
            "Workflow" => Ok(RunTrigger::Workflow),
            other => Err(format!(
                "invalid run trigger '{}': expected Manual, Scheduled, Cue or Workflow",
                other
            )),
        }
    }
}

impl Serialize for RunTrigger {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunTrigger {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
