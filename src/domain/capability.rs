//! The frozen peer-capability vocabulary shared by enrollment, the trust
//! registry, Cues, baseline pushes and the Health Plane.

pub const CAPABILITY_BACKUP_ORCHESTRATION: &str = "backup-orchestration";
pub const CAPABILITY_BASELINE_PUSH: &str = "baseline-push";
pub const CAPABILITY_INVENTORY_HEALTH: &str = "inventory-health";
pub const CAPABILITY_LOST_DEVICE_REVOCATION: &str = "lost-device-revocation";
pub const CAPABILITY_NOTIFICATIONS: &str = "notifications";
pub const CAPABILITY_REMOTE_RUN: &str = "remote-run";
pub const CAPABILITY_SSH_CREDENTIAL_ROTATION: &str = "ssh-credential-rotation";

/// Every capability a peer may be granted, sorted.
pub const CAPABILITY_ALLOWLIST: [&str; 7] = [
    CAPABILITY_BACKUP_ORCHESTRATION,
    CAPABILITY_BASELINE_PUSH,
    CAPABILITY_INVENTORY_HEALTH,
    CAPABILITY_LOST_DEVICE_REVOCATION,
    CAPABILITY_NOTIFICATIONS,
    CAPABILITY_REMOTE_RUN,
    CAPABILITY_SSH_CREDENTIAL_ROTATION,
];

/// The most capabilities one peer may hold.
pub const MAX_CAPABILITIES: usize = 32;
/// The longest capability name a wire frame may carry.
pub const MAX_CAPABILITY_BYTES: usize = 64;

/// Why a capability list is not one a peer may hold.
#[derive(Debug, PartialEq, Eq)]
pub enum CapabilityListError<'a> {
    TooMany,
    Unsupported(&'a str),
    Unsorted,
}

/// Check a peer's capability list: at most [`MAX_CAPABILITIES`] entries, each
/// on the allow-list, sorted and unique. Allow-list membership implies the
/// capability name grammar and length bound.
pub fn check_capability_list<S: AsRef<str>>(
    capabilities: &[S],
) -> Result<(), CapabilityListError<'_>> {
    if capabilities.len() > MAX_CAPABILITIES {
        return Err(CapabilityListError::TooMany);
    }
    let mut previous: Option<&str> = None;
    for capability in capabilities {
        let capability = capability.as_ref();
        if !CAPABILITY_ALLOWLIST.contains(&capability) {
            return Err(CapabilityListError::Unsupported(capability));
        }
        if previous.is_some_and(|previous| previous >= capability) {
            return Err(CapabilityListError::Unsorted);
        }
        previous = Some(capability);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allow_list_is_sorted_and_unique() {
        assert!(CAPABILITY_ALLOWLIST
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(CAPABILITY_ALLOWLIST
            .iter()
            .all(|capability| capability.len() <= MAX_CAPABILITY_BYTES));
    }

    #[test]
    fn a_capability_list_is_bounded_known_sorted_and_unique() {
        assert_eq!(check_capability_list(&CAPABILITY_ALLOWLIST), Ok(()));
        assert_eq!(check_capability_list::<&str>(&[]), Ok(()));
        assert_eq!(
            check_capability_list(&["remote-run", "Remote-Run"]),
            Err(CapabilityListError::Unsupported("Remote-Run"))
        );
        assert_eq!(
            check_capability_list(&["remote-run", "baseline-push"]),
            Err(CapabilityListError::Unsorted)
        );
        assert_eq!(
            check_capability_list(&["remote-run", "remote-run"]),
            Err(CapabilityListError::Unsorted)
        );
        assert_eq!(
            check_capability_list(&vec!["remote-run"; MAX_CAPABILITIES + 1]),
            Err(CapabilityListError::TooMany)
        );
    }
}
