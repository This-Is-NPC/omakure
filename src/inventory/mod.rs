//! Structural inventories for CLI commands and HTTP management routes.
//!
//! The command inventory reads the Clap tree declared in `cli::args`; parity
//! and catalog consumers depend only on this module.

mod command;
mod routes;

pub(crate) use command::{InventoryCommand, InventoryOption, command_inventory};
pub use command::{normalize_generated_text, render_cli_reference};
pub use routes::HTTP_ROUTE_INVENTORY;
