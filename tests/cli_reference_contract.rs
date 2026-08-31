use omakure::cli::inventory::render_cli_reference;

const REFERENCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/docs/cli-reference.md"
));
const USAGE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/usage.md"));

#[test]
fn generated_reference_matches_clap_inventory_byte_for_byte() {
    assert_eq!(REFERENCE, render_cli_reference());
    assert!(REFERENCE.starts_with("<!-- BEGIN GENERATED CLI REFERENCE -->\n"));
    assert!(REFERENCE.ends_with("<!-- END GENERATED CLI REFERENCE -->\n"));
}

#[test]
fn reference_contains_nested_commands_and_new_metadata() {
    for id in [
        "node baseline publish",
        "node enroll approve",
        "node authority issue",
        "queue add",
        "battery add",
    ] {
        assert!(
            REFERENCE.contains(&format!("- **ID:** `{id}`")),
            "missing {id}"
        );
    }
    for token in [
        "--run-id",
        "--token-ref",
        "**Aliases:** `check`",
        "--lifetime-seconds",
    ] {
        assert!(REFERENCE.contains(token), "missing {token}");
    }
}

#[test]
fn usage_sections_have_required_operational_contract() {
    let sections = [
        ("node direct-probe", "--endpoint", "--peer-node-id"),
        ("node init", "omakure node init", "--scripts-dir"),
        ("node discovery", "--wait-seconds", "--include-addresses"),
        ("node trust", "--public-key", "--confirmed"),
        ("node capabilities", "--capability", "--confirmed"),
        ("completion", "omakure completion bash", "SHELL"),
    ];
    for (section, command, flag) in sections {
        let anchor = format!("## `omakure {section}`");
        let start = USAGE
            .find(&anchor)
            .unwrap_or_else(|| panic!("missing {anchor}"));
        let body = &USAGE[start..]
            .split_once("\n## ")
            .map(|(body, _)| body)
            .unwrap_or(&USAGE[start..]);
        assert!(body.contains("**Synopsis:**"), "{section} lacks synopsis");
        assert!(
            body.contains("```bash"),
            "{section} lacks executable example"
        );
        assert!(body.contains(command), "{section} lacks command {command}");
        assert!(body.contains(flag), "{section} lacks relevant flag {flag}");
        assert!(
            body.contains("principal failure"),
            "{section} lacks failure mode"
        );
        assert!(
            body.contains("generated CLI reference"),
            "{section} lacks reference link"
        );
    }
}
