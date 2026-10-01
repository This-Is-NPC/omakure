mod adapters;
mod app_meta;
pub(crate) mod auth;
/// The signed, versioned baseline artefact: the first thing this product puts
/// on a node that is code rather than an order.
pub mod baseline;
/// Custody of the key that signs baselines, held apart from the key that
/// enrols members.
pub(crate) mod baseline_publisher;
/// Deciding whether another node may put code on this one, and installing it
/// when the answer is yes.
pub mod baseline_push;
pub mod cli;
/// The versioned, generated contract describing CLI and HTTP parity.
pub mod cli_http_parity;
pub mod direct_health;
pub mod direct_service;
pub mod direct_transport;
pub mod discovery;
pub mod domain;
pub mod enrollment;
pub(crate) mod enrollment_authority;
mod error;
pub mod health_plane;
/// Shared structural inventories for generated contracts.
pub mod inventory;
pub mod node;
pub mod node_identity;
mod node_key;
pub mod node_registry;
pub mod node_transport;
/// The canonical versioned operation metadata catalog.
pub mod operation_catalog;
pub mod operations;
mod policy;
mod ports;
pub(crate) mod redaction;
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
pub use runs::{HEARTBEAT_MS, RunTrigger};
mod runtime;
/// The two constants the binary needs to enter embedded-Lua host mode.
///
/// Re-exported narrowly rather than making `runtime` public: nothing else in
/// the module is part of the crate's contract.
pub use runtime::{LUA_HOST_ARG, LUA_HOST_FAILURE_EXIT};
mod search_index;
pub(crate) mod secrets;
#[cfg(test)]
mod test_support;
mod util;
#[doc(hidden)]
pub use util::exec::{generated_executable_tempdir, write_generated_executable};
#[doc(hidden)]
pub use util::hex;
mod workspace;
