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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allow_list_is_sorted_and_unique() {
        assert!(CAPABILITY_ALLOWLIST
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
    }
}
