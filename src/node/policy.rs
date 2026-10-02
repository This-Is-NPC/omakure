use super::context::NodeContext;
use crate::domain::NodeConfig;
use std::io::Read;

/// Why a policy read produced the configuration it did.
///
/// Every variant below denies everything, and that is correct: a node that
/// cannot prove what it opted into has opted into nothing. But "nothing was
/// declared" and "the config exists and this node could not read it" are the
/// same *decision* and completely different operator problems. Collapsing them
/// is how one mode bit silently disables remote Cues, or the baseline gate,
/// with no distinguishable reason.
pub(crate) enum PolicyConfig {
    /// The config was read and parsed. Whatever it declares is what holds.
    Declared(Box<NodeConfig>),
    /// There is no config file. Nothing was declared.
    NothingDeclared,
    /// A config exists and could not be read or trusted. `String` says why, in
    /// the terms the operator needs: which file, and what is wrong with it.
    Unreadable(String),
}

/// Read this node's own public config for a policy decision, keeping the
/// reason for any failure rather than discarding it.
pub(crate) fn read_policy_config(context: &NodeContext) -> PolicyConfig {
    let mut file = match context.open_public_file() {
        Ok(Some(file)) => file,
        Ok(None) => return PolicyConfig::NothingDeclared,
        Err(error) => return PolicyConfig::Unreadable(error.to_string()),
    };
    let mut contents = String::new();
    if let Err(error) = file.read_to_string(&mut contents) {
        return PolicyConfig::Unreadable(format!(
            "{} could not be read: {error}",
            context.config_path().display()
        ));
    }
    match NodeConfig::parse(&contents) {
        Ok(config) => PolicyConfig::Declared(Box::new(config)),
        Err(error) => PolicyConfig::Unreadable(format!(
            "{} is not a usable node configuration: {error}",
            context.config_path().display()
        )),
    }
}

/// Report an unreadable policy config once per distinct reason.
///
/// Policy is read per session so a change takes effect without a restart, so an
/// unconditional warning would repeat for every inbound connection.
/// Deduplicating on the reason keeps a standing misconfiguration to one line
/// while still reporting a *new* problem when one appears.
pub(crate) fn warn_policy_unreadable(gate: &str, reason: &str) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};

    static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut reported = match reported.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if reported.insert(format!("{gate}\u{0}{reason}")) {
        eprintln!(
            "omakure: {gate} denied for every peer; this node's configuration could not be read ({reason})"
        );
    }
}
