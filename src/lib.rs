mod adapters;
mod app_meta;
pub mod auth;
/// The signed, versioned baseline artefact: the first thing this product puts
/// on a node that is code rather than an order.
pub mod baseline;
pub mod cli;
pub mod direct_health;
pub mod direct_service;
pub mod direct_transport;
pub mod discovery;
pub mod domain;
pub mod enrollment;
pub mod enrollment_authority;
mod error;
pub mod health_plane;
pub mod node;
pub mod node_identity;
pub mod node_registry;
pub mod node_transport;
pub mod operations;
mod policy;
mod ports;
pub mod redaction;
/// The receive half of the Remote Cue plane: authorization only, no execution.
pub mod remote_cue;
mod run_executor;
mod runs;
/// Run provenance and the lease window, needed by the frozen Remote Cue
/// contract.
///
/// Re-exported narrowly rather than making `runs` public. The contract pins
/// both against the shipped values on purpose: a frozen number that lives only
/// in a fixture is a decoupled constant, and drifting from the code it claims
/// to describe is exactly how such a number stops meaning anything.
pub use runs::{RunTrigger, HEARTBEAT_MS};
mod runtime;
/// The two constants the binary needs to enter embedded-Lua host mode.
///
/// Re-exported narrowly rather than making `runtime` public: nothing else in
/// the module is part of the crate's contract.
pub use runtime::{LUA_HOST_ARG, LUA_HOST_FAILURE_EXIT};
mod search_index;
pub mod secrets;
mod use_cases;
mod util;
mod workspace;
