//! Domain layer - core types and validation logic.

mod capability;
pub mod health_plane;
mod node_config;
mod node_id;
mod parsing;
mod schedule;
mod schema;

pub use capability::{
    CAPABILITY_ALLOWLIST, CAPABILITY_BASELINE_PUSH, CAPABILITY_INVENTORY_HEALTH,
    CAPABILITY_NOTIFICATIONS, CAPABILITY_REMOTE_RUN, CapabilityListError, MAX_CAPABILITIES,
    MAX_CAPABILITY_BYTES, check_capability_list,
};
pub use node_config::{
    DiscoverySettings, EnrollmentAuthority, NodeConfig, NodeConfigError, TrustedBaselinePublisher,
    parse_node_config,
};
pub use node_id::{NODE_ID_BYTES, NODE_ID_PREFIX, is_node_id};
pub use parsing::{extract_schema_block, parse_schema};
pub use schedule::{next_fire_after, parse_cron};
pub use schema::Schema;
