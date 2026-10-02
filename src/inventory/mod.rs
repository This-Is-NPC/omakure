//! Structural inventories for CLI commands and HTTP management routes.
//!
//! The command inventory converts a supplied Clap tree; the CLI adapter owns
//! construction of that tree. Parity and catalog consumers receive its IDs.

mod command;
mod routes;

pub use command::normalize_generated_text;
pub(crate) use command::{
    InventoryCommand, InventoryOption, command_inventory, render_cli_reference,
};
pub use routes::HTTP_ROUTE_INVENTORY;
