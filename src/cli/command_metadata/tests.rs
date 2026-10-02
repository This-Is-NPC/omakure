use super::*;
use crate::inventory::normalize_generated_text;

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
fn generated_reference_freshness_treats_crlf_checkout_as_lf() {
    let generated = render_cli_reference();
    let crlf = generated.replace('\n', "\r\n");
    assert_ne!(crlf, generated);
    assert_eq!(normalize_generated_text(&crlf), generated);
}

#[test]
fn two_reference_generations_are_identical() {
    assert_eq!(render_cli_reference(), render_cli_reference());
}

#[test]
fn two_inventory_builds_are_equal() {
    assert_eq!(command_inventory(), command_inventory());
}

#[test]
fn live_cli_inventory_satisfies_parity_and_catalog() {
    let cli_ids = current_cli_ids();
    assert_eq!(cli_ids.len(), 65);

    let parity =
        crate::cli_http_parity::validate_current(&cli_ids).expect("valid live parity inventory");
    assert_eq!(parity.entries.len(), 72);

    let catalog = crate::operation_catalog::validate_current(&cli_ids)
        .expect("valid operation catalog for live CLI inventory");
    assert_eq!(catalog.operations.len(), parity.entries.len());
}
