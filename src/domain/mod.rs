//! Domain layer - core types and validation logic.

mod capability;
mod node_config;
mod node_id;
mod parsing;
mod schedule;
mod schema;

pub use capability::{
    check_capability_list, CapabilityListError, CAPABILITY_ALLOWLIST, CAPABILITY_BASELINE_PUSH,
    CAPABILITY_INVENTORY_HEALTH, CAPABILITY_NOTIFICATIONS, CAPABILITY_REMOTE_RUN, MAX_CAPABILITIES,
    MAX_CAPABILITY_BYTES,
};
pub use node_config::{
    parse_node_config, DiscoverySettings, EnrollmentAuthority, NodeConfig, NodeConfigError,
    TrustedBaselinePublisher,
};
pub use node_id::{is_node_id, NODE_ID_BYTES, NODE_ID_PREFIX};
pub use parsing::{extract_schema_block, parse_schema};
pub use schedule::{next_fire_after, parse_cron};
pub use schema::Schema;
