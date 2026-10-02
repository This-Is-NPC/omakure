/// Stable rejection codes, frozen in `docs/internal/remote-cue-contract.md`.
///
/// The band is `1201..` inside the existing `transport_audit.error_code` range
/// `1000..=1999`, disjoint from transport `1001..=1011`/`1020` and Health
/// `1101..=1115`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueCode {
    Disabled,
    NotActiveConductor,
    MissingRemoteRun,
    MissingNotifications,
    NotDeclared,
    ScriptDeclaresSecrets,
    ScriptUnresolvable,
    Expired,
    Duplicate,
    RateLimited,
    RunAlreadyInFlight,
    InvalidMessage,
}

impl CueCode {
    /// The stable code, typed to the width of the `transport_audit` column it
    /// is written to so no cast can silently truncate it.
    pub fn code(self) -> u16 {
        match self {
            CueCode::Disabled => 1201,
            CueCode::NotActiveConductor => 1202,
            CueCode::MissingRemoteRun => 1203,
            CueCode::MissingNotifications => 1204,
            CueCode::NotDeclared => 1212,
            CueCode::ScriptDeclaresSecrets => 1205,
            CueCode::ScriptUnresolvable => 1206,
            CueCode::Expired => 1207,
            CueCode::Duplicate => 1208,
            CueCode::RateLimited => 1209,
            CueCode::RunAlreadyInFlight => 1210,
            CueCode::InvalidMessage => 1211,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CueCode::Disabled => "cue_disabled",
            CueCode::NotActiveConductor => "cue_not_active_conductor",
            CueCode::MissingRemoteRun => "cue_missing_remote_run",
            CueCode::MissingNotifications => "cue_missing_notifications",
            CueCode::NotDeclared => "cue_script_not_declared",
            CueCode::ScriptDeclaresSecrets => "cue_script_declares_secrets",
            CueCode::ScriptUnresolvable => "cue_script_unresolvable",
            CueCode::Expired => "cue_expired",
            CueCode::Duplicate => "cue_duplicate",
            CueCode::RateLimited => "cue_rate_limited",
            CueCode::RunAlreadyInFlight => "cue_run_already_in_flight",
            CueCode::InvalidMessage => "cue_invalid_message",
        }
    }

    /// The code this refusal is *reported* as, which is not always the code it
    /// is *audited* as.
    ///
    /// `NotDeclared` is audited distinctly, because the operator of the
    /// receiving node genuinely wants to know that someone asked for a script
    /// they never declared. It is reported as `ScriptUnresolvable`, because
    /// telling an authorized Conductor the difference between "exists but is
    /// not declared" and "does not exist" lets it enumerate the workspace by
    /// elimination — the same oracle the contract already closed by collapsing
    /// missing and ignored into one code.
    pub fn reply_code(self) -> CueCode {
        match self {
            CueCode::NotDeclared => CueCode::ScriptUnresolvable,
            other => other,
        }
    }

    /// Whether a refusal with this code may be told to the sender.
    ///
    /// Follows the Health Plane precedent: trust, role, and capability failures
    /// are dropped and audited only, so an unauthorized peer learns nothing —
    /// not even that remote Cues exist on this node. Everything else is a
    /// message the sender is already authorized to have evaluated.
    pub fn is_reportable(self) -> bool {
        !matches!(
            self,
            CueCode::Disabled
                | CueCode::NotActiveConductor
                | CueCode::MissingRemoteRun
                | CueCode::MissingNotifications
        )
    }
}
