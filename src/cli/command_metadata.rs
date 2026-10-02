//! Adapter from the live Clap command tree to neutral inventory projections.

use crate::cli::args::Cli;
use crate::inventory::{self, InventoryCommand};
use clap::CommandFactory;

pub(crate) fn command_inventory() -> Vec<InventoryCommand> {
    inventory::command_inventory(&Cli::command())
}

pub fn current_cli_ids() -> Vec<String> {
    command_inventory()
        .into_iter()
        .filter(|command| command.subcommands.is_empty())
        .map(|command| command.id)
        .collect()
}

pub fn render_cli_reference() -> String {
    inventory::render_cli_reference(&command_inventory())
}

#[cfg(test)]
mod tests;
