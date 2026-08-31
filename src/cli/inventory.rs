//! Deterministic structural inventory of the Clap command tree.
//!
//! This is the one structural source shared by `help-ai`, the generated CLI
//! reference, and parity tooling.  It deliberately stores metadata rather than
//! rendering help text so consumers can choose their own presentation.

use crate::cli::args::Cli;
use clap::CommandFactory;
use serde::Serialize;

/// A command (including intermediate commands and leaves) in the public Clap
/// tree.  `id` is the canonical full command path, excluding the binary name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct InventoryCommand {
    /// Stable full command path, for example `node baseline enroll`.
    pub(crate) id: String,
    /// Components of [`id`], retained to avoid reparsing paths.
    pub(crate) path: Vec<String>,
    pub(crate) name: String,
    pub(crate) about: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) hidden_aliases: Vec<String>,
    pub(crate) hidden: bool,
    /// Clap 4 has no command-level deprecation bit; this remains explicit so
    /// the schema can represent it if the parser gains one without changing
    /// consumers.
    pub(crate) deprecated: bool,
    pub(crate) options: Vec<InventoryOption>,
    /// Canonical IDs of direct child commands, in deterministic order.
    pub(crate) subcommands: Vec<String>,
}

/// An option or positional argument declared on a command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct InventoryOption {
    pub(crate) id: String,
    pub(crate) long: Option<String>,
    pub(crate) short: Option<char>,
    pub(crate) aliases: Vec<String>,
    pub(crate) hidden_aliases: Vec<String>,
    pub(crate) help: String,
    pub(crate) positional: bool,
    pub(crate) takes_value: bool,
    pub(crate) required: bool,
    pub(crate) default_values: Vec<String>,
    pub(crate) value_names: Vec<String>,
    pub(crate) possible_values: Vec<InventoryPossibleValue>,
    pub(crate) min_values: Option<usize>,
    pub(crate) max_values: Option<usize>,
    pub(crate) action: String,
    pub(crate) global: bool,
    pub(crate) hidden: bool,
    /// Clap 4 has no argument-level deprecation bit; see
    /// [`InventoryCommand::deprecated`].
    pub(crate) deprecated: bool,
}

/// One constrained value accepted by an option's value parser.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct InventoryPossibleValue {
    pub(crate) name: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) help: String,
    pub(crate) hidden: bool,
}

/// Build the complete deterministic inventory from `Cli::command()`.
///
/// The command tree is traversed recursively.  Every command and argument is
/// sorted by its canonical name after collection, making output independent of
/// declaration order while retaining the canonical full-path IDs.
pub(crate) fn command_inventory() -> Vec<InventoryCommand> {
    let root = Cli::command();
    let inherited: Vec<InventoryOption> = root
        .get_arguments()
        .filter(|argument| argument.is_global_set())
        .map(inventory_option)
        .collect();
    let mut entries = Vec::new();
    let mut path = Vec::new();
    collect_commands(&root, &mut path, &mut entries, &inherited);
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries
}

fn collect_commands(
    command: &clap::Command,
    path: &mut Vec<String>,
    entries: &mut Vec<InventoryCommand>,
    inherited: &[InventoryOption],
) {
    for child in command.get_subcommands() {
        path.push(child.get_name().to_string());
        let id = path.join(" ");
        let mut subcommands: Vec<String> = child
            .get_subcommands()
            .map(|sub| format!("{} {}", id, sub.get_name()))
            .collect();
        subcommands.sort();

        let mut options: Vec<InventoryOption> =
            child.get_arguments().map(inventory_option).collect();
        options.extend(inherited.iter().cloned());
        options.sort_by_key(option_sort_key);

        let mut aliases: Vec<String> = child.get_visible_aliases().map(str::to_owned).collect();
        aliases.sort();
        let mut hidden_aliases: Vec<String> = child.get_aliases().map(str::to_owned).collect();
        hidden_aliases.sort();

        entries.push(InventoryCommand {
            id,
            path: path.clone(),
            name: child.get_name().to_string(),
            about: child
                .get_about()
                .map(ToString::to_string)
                .unwrap_or_default(),
            aliases,
            hidden_aliases,
            hidden: child.is_hide_set(),
            deprecated: false,
            options,
            subcommands,
        });

        collect_commands(child, path, entries, inherited);
        path.pop();
    }
}

fn option_sort_key(option: &InventoryOption) -> (bool, String, String) {
    (
        option.positional,
        option.long.clone().unwrap_or_default(),
        option.id.clone(),
    )
}

fn inventory_option(argument: &clap::Arg) -> InventoryOption {
    let mut aliases: Vec<String> = argument
        .get_visible_aliases()
        .unwrap_or_default()
        .into_iter()
        .map(str::to_owned)
        .collect();
    aliases.sort();
    let mut hidden_aliases: Vec<String> = argument
        .get_aliases()
        .unwrap_or_default()
        .into_iter()
        .map(str::to_owned)
        .collect();
    hidden_aliases.sort();

    let value_range = argument.get_num_args();
    let takes_value = argument.get_action().takes_values();
    let (min_values, max_values) = value_range
        .map(|range| (Some(range.min_values()), Some(range.max_values())))
        .unwrap_or_else(|| {
            if takes_value {
                // Clap's value-taking actions accept exactly one value when
                // no explicit range is configured.
                (Some(1), Some(1))
            } else {
                (None, None)
            }
        });

    let mut possible_values: Vec<InventoryPossibleValue> = argument
        .get_possible_values()
        .into_iter()
        .map(|value| {
            let mut names = value
                .get_name_and_aliases()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let name = names.remove(0);
            names.sort();
            InventoryPossibleValue {
                name,
                aliases: names,
                help: value
                    .get_help()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                hidden: value.is_hide_set(),
            }
        })
        .collect();
    possible_values.sort_by(|a, b| a.name.cmp(&b.name));

    InventoryOption {
        id: argument.get_id().to_string(),
        long: argument.get_long().map(str::to_owned),
        short: argument.get_short(),
        aliases,
        hidden_aliases,
        help: argument
            .get_help()
            .map(ToString::to_string)
            .unwrap_or_default(),
        positional: argument.is_positional(),
        takes_value,
        required: argument.is_required_set(),
        default_values: argument
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect(),
        value_names: argument
            .get_value_names()
            .unwrap_or_default()
            .iter()
            .map(ToString::to_string)
            .collect(),
        possible_values,
        min_values,
        max_values,
        action: format!("{:?}", argument.get_action()),
        global: argument.is_global_set(),
        hidden: argument.is_hide_set(),
        deprecated: false,
    }
}

/// Render the generated CLI reference body from the same inventory consumed by
/// `help-ai`.  The output is deterministic and contains no environment data.
pub fn render_cli_reference() -> String {
    let mut output = reference_header();
    for command in command_inventory() {
        render_command(&mut output, &command);
    }
    output.push_str("<!-- END GENERATED CLI REFERENCE -->\n");
    output
}

fn reference_header() -> String {
    String::from(
        "<!-- BEGIN GENERATED CLI REFERENCE -->\n\
         # CLI reference\n\n\
         This section is generated from Clap metadata. Run `cargo run --bin cli-reference` to refresh it.\n\n",
    )
}

fn render_command(output: &mut String, command: &InventoryCommand) {
    use std::fmt::Write as _;

    let _ = writeln!(output, "## `omakure {}`", command.id);
    render_command_details(output, command);
    render_options(output, command);
    render_subcommands(output, command);
    output.push('\n');
}

fn render_command_details(output: &mut String, command: &InventoryCommand) {
    use std::fmt::Write as _;

    if !command.about.is_empty() {
        let _ = writeln!(output, "\n{}", command.about);
    }
    let _ = writeln!(output, "\n- **ID:** `{}`", command.id);
    render_aliases(output, command);
    let _ = writeln!(
        output,
        "- **Visibility:** {}{}",
        if command.hidden { "hidden" } else { "visible" },
        if command.deprecated {
            ", deprecated"
        } else {
            ""
        }
    );
}

fn render_aliases(output: &mut String, command: &InventoryCommand) {
    use std::fmt::Write as _;

    if !command.aliases.is_empty() {
        let _ = writeln!(output, "- **Aliases:** `{}`", command.aliases.join("`, `"));
    }
    if !command.hidden_aliases.is_empty() {
        let _ = writeln!(
            output,
            "- **Hidden aliases:** `{}`",
            command.hidden_aliases.join("`, `")
        );
    }
}

fn render_options(output: &mut String, command: &InventoryCommand) {
    if !command.options.iter().any(|option| !option.hidden) {
        return;
    }
    output.push_str("\n### Options\n\n");
    for option in command.options.iter().filter(|option| !option.hidden) {
        render_option(output, option);
    }
}

fn render_option(output: &mut String, option: &InventoryOption) {
    use std::fmt::Write as _;

    let syntax = option_syntax(option);
    let _ = write!(output, "- `{syntax}`");
    if !option.help.is_empty() {
        let _ = write!(output, " — {}", option.help);
    }
    if option.required {
        output.push_str(" **(required)**");
    }
    if !option.default_values.is_empty() {
        let _ = write!(
            output,
            " (default: `{}`)",
            option.default_values.join("`, `")
        );
    }
    if !option.possible_values.is_empty() {
        let values = option
            .possible_values
            .iter()
            .map(|value| value.name.as_str())
            .collect::<Vec<_>>()
            .join("`, `");
        let _ = write!(output, " (values: `{values}`)");
    }
    output.push('\n');
}

fn render_subcommands(output: &mut String, command: &InventoryCommand) {
    use std::fmt::Write as _;

    if command.subcommands.is_empty() {
        return;
    }
    output.push_str("\n### Subcommands\n\n");
    for subcommand in &command.subcommands {
        let _ = writeln!(output, "- [`{subcommand}`](#{})", anchor(subcommand));
    }
}

fn option_syntax(option: &InventoryOption) -> String {
    let mut syntax = String::new();
    if let Some(short) = option.short {
        syntax.push('-');
        syntax.push(short);
        if option.long.is_some() {
            syntax.push_str(", ");
        }
    }
    if let Some(long) = &option.long {
        syntax.push_str("--");
        syntax.push_str(long);
    }
    if option.positional {
        syntax.push_str(
            option
                .value_names
                .first()
                .map(String::as_str)
                .unwrap_or("<VALUE>"),
        );
    } else if option.takes_value {
        syntax.push(' ');
        syntax.push_str(
            option
                .value_names
                .first()
                .map(String::as_str)
                .unwrap_or("<VALUE>"),
        );
    }
    if syntax.is_empty() {
        syntax.push_str(&option.id);
    }
    syntax
}

fn anchor(value: &str) -> String {
    format!("omakure-{}", value.replace(' ', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_is_sorted_and_has_stable_full_paths() {
        let inventory = command_inventory();
        assert!(!inventory.is_empty());
        let ids: Vec<&str> = inventory.iter().map(|entry| entry.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
        assert!(ids.contains(&"node baseline"));
        assert!(ids.contains(&"node enroll approve"));
        assert!(ids.contains(&"node authority issue"));
        assert!(ids.contains(&"queue add"));
    }

    #[test]
    fn inventory_carries_defaults_constraints_aliases_and_hidden_metadata() {
        let inventory = command_inventory();
        let queue_add = inventory
            .iter()
            .find(|entry| entry.id == "queue add")
            .unwrap();
        let actor = queue_add
            .options
            .iter()
            .find(|option| option.long.as_deref() == Some("actor"))
            .unwrap();
        assert_eq!(actor.default_values, ["human"]);
        assert!(actor.takes_value);
        assert_eq!(actor.max_values, Some(1));

        let issue = inventory
            .iter()
            .find(|entry| entry.id == "node authority issue")
            .unwrap();
        let role = issue
            .options
            .iter()
            .find(|option| option.long.as_deref() == Some("role"))
            .unwrap();
        assert_eq!(
            role.possible_values
                .iter()
                .map(|value| value.name.as_str())
                .collect::<Vec<_>>(),
            ["conductor", "performer"]
        );
        assert_eq!(role.min_values, Some(1));
        assert_eq!(role.max_values, Some(1));

        let worker = inventory
            .iter()
            .find(|entry| entry.id == "queue worker")
            .unwrap();
        let once = worker
            .options
            .iter()
            .find(|option| option.long.as_deref() == Some("once"))
            .unwrap();
        assert!(once.hidden);

        let doctor = inventory.iter().find(|entry| entry.id == "doctor").unwrap();
        assert!(doctor.aliases.iter().any(|alias| alias == "check"));
    }

    #[test]
    fn reference_links_match_command_heading_slugs() {
        let reference = render_cli_reference();
        assert!(reference.contains("- [`battery add`](#omakure-battery-add)"));
        assert!(!reference.contains("- [`battery add`](#battery-add)"));
    }

    #[test]
    fn reference_omits_hidden_options() {
        let reference = render_cli_reference();
        assert!(!reference.contains("--once"));
    }

    #[test]
    fn every_reference_subcommand_link_resolves() {
        let reference = render_cli_reference();
        for command in command_inventory() {
            for subcommand in command.subcommands {
                assert!(reference.contains(&format!("## `omakure {subcommand}`")));
                assert!(
                    reference.contains(&format!("- [`{subcommand}`](#{})", anchor(&subcommand)))
                );
            }
        }
    }

    #[test]
    fn two_reference_generations_are_identical() {
        assert_eq!(render_cli_reference(), render_cli_reference());
    }

    #[test]
    fn two_inventory_builds_are_equal() {
        assert_eq!(command_inventory(), command_inventory());
    }
}
