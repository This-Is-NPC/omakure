use omakure::cli::inventory::{normalize_generated_text, render_cli_reference};
use omakure::cli_http_parity::{check_docs_freshness, checked_manifest};
use std::fs;
use std::path::{Path, PathBuf};

const REFERENCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/docs/cli-reference.md"
));
const USAGE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/usage.md"));
const DOCS_INDEX: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/README.md"));
const FLEET: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/docs/fleet-operations.md"
));
const PARITY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/docs/cli-http-parity.md"
));
const CHANGED_PUBLIC_DOCS: &[&str] = &[
    "README.md",
    "docs/README.md",
    "docs/usage.md",
    "docs/fleet-operations.md",
    "docs/http-api.md",
    "docs/deployment.md",
    "docs/ai-interface.md",
];

const AUDITED_TEMPORAL_PHRASES: &[&str] = &[
    "control-roadmap.md",
    "## roadmap",
    "## product direction",
    "optional later:",
    "remain future features",
    "deliberately deferred to task",
    "for this wave",
    "wave 2.",
    "excluded from this plan",
    "pre-implementation",
    "future e2e tests",
    "every later direct-channel",
    "reserved for future use",
    "roadmap item",
    "task #",
    "wave ",
    "preimplementation",
];

const REFERENCE_TARGETS: &[&str] = &[
    "cli-reference.md",
    "usage/omakure.md",
    "usage/omakure.1",
    "usage/omakure.kdl",
    "operation-catalog.md",
    "operation-support-matrix.md",
    "cli-http-parity.md",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn markdown_files_under(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read documentation directory {directory:?}: {error}"))
    {
        let entry = entry.expect("read documentation directory entry");
        let path = entry.path();
        if path.is_dir() {
            markdown_files_under(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn docs_section<'a>(document: &'a str, heading: &str) -> &'a str {
    let start = document
        .find(heading)
        .unwrap_or_else(|| panic!("missing documentation heading {heading:?}"));
    let body = &document[start..];
    body.split_once("\n## ")
        .map(|(body, _)| body)
        .unwrap_or(body)
}

fn markdown_targets(document: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut fenced = false;
    for line in document.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut remaining = line;
        while let Some(open) = remaining.find('[') {
            let label = &remaining[open + 1..];
            let Some(close) = label.find("](") else {
                break;
            };
            let raw_target = &label[close + 2..];
            let Some(end) = raw_target.find(')') else {
                break;
            };
            let target = markdown_destination(&raw_target[..end]).trim_matches('<');
            if !target.is_empty() {
                targets.push(target.to_string());
            }
            remaining = &raw_target[end + 1..];
        }
    }
    targets
}

fn markdown_destination(raw_target: &str) -> &str {
    let raw_target = raw_target.trim();
    if let Some(bracketed) = raw_target.strip_prefix('<') {
        bracketed
            .split_once('>')
            .map_or(bracketed, |(destination, _)| destination)
    } else {
        raw_target.split_whitespace().next().unwrap_or("")
    }
}

fn is_windows_drive_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(format!("truncated escape at byte {index}"));
        }
        let high = (bytes[index + 1] as char)
            .to_digit(16)
            .ok_or_else(|| format!("invalid escape at byte {index}"))?;
        let low = (bytes[index + 2] as char)
            .to_digit(16)
            .ok_or_else(|| format!("invalid escape at byte {index}"))?;
        decoded.push((high * 16 + low) as u8);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|error| format!("invalid UTF-8 escape: {error}"))
}

fn decode_path(path: &str) -> Result<String, String> {
    path.split('/')
        .map(percent_decode)
        .collect::<Result<Vec<_>, _>>()
        .map(|segments| segments.join("/"))
}

fn uri_scheme(target: &str) -> Option<&str> {
    let colon = target.find(':')?;
    let scheme = &target[..colon];
    if scheme.is_empty()
        || !scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic()
                || index > 0 && byte.is_ascii_alphanumeric()
                || index > 0 && matches!(byte, b'+' | b'-' | b'.')
        })
    {
        return None;
    }
    Some(scheme)
}

fn github_heading_anchor(heading: &str) -> String {
    let heading = heading.trim().trim_end_matches('#').trim();
    let mut anchor = String::new();
    let mut pending_dash = false;
    for character in heading.chars() {
        if character.is_alphanumeric() {
            if pending_dash && !anchor.is_empty() {
                anchor.push('-');
            }
            pending_dash = false;
            anchor.extend(character.to_lowercase());
        } else if character == '-' || character.is_whitespace() {
            pending_dash = true;
        }
    }
    anchor
}

fn document_anchors(document: &str) -> Vec<String> {
    let mut anchors = Vec::new();
    let mut fenced = false;
    let mut heading_counts = std::collections::HashMap::<String, usize>::new();
    for line in document.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some(heading) = trimmed
            .strip_prefix('#')
            .filter(|_| trimmed.starts_with("##") || trimmed.starts_with("# "))
        {
            let base = github_heading_anchor(heading);
            let count = heading_counts.entry(base.clone()).or_default();
            let anchor = if *count == 0 {
                base.clone()
            } else {
                format!("{base}-{count}")
            };
            *count += 1;
            anchors.push(anchor);
        }
        if !line.contains('<') {
            continue;
        }
        for attribute in ["id", "name"] {
            for quote in ['"', '\''] {
                let needle = format!("{attribute}={quote}");
                let mut rest = line;
                while let Some(start) = rest.find(&needle) {
                    let value = &rest[start + needle.len()..];
                    let Some(end) = value.find(quote) else {
                        break;
                    };
                    anchors.push(value[..end].to_string());
                    rest = &value[end + 1..];
                }
            }
        }
    }
    anchors
}

fn assert_anchor_exists(document: &str, fragment: &str, target: &str) {
    let fragment = percent_decode(fragment)
        .unwrap_or_else(|error| panic!("invalid percent-encoded fragment in {target:?}: {error}"));
    assert!(
        document_anchors(document)
            .iter()
            .any(|anchor| anchor == &fragment),
        "link {target:?} points to missing anchor #{fragment}"
    );
}
fn assert_repository_relative_links_are_resolved() {
    let root = repo_root();
    let canonical_root = root.canonicalize().expect("canonicalize repository root");
    for relative_doc in CHANGED_PUBLIC_DOCS {
        let document_path = root.join(relative_doc);
        let document = fs::read_to_string(&document_path)
            .unwrap_or_else(|error| panic!("read {relative_doc}: {error}"));
        let parent = document_path.parent().expect("documentation parent");
        for target in markdown_targets(&document) {
            if is_windows_drive_absolute(&target) {
                panic!("{relative_doc} has an absolute link {target:?}");
            }
            if target.starts_with("//") || uri_scheme(&target).is_some() {
                continue;
            }
            let (raw_path, raw_fragment) = target
                .split_once('#')
                .map_or((target.as_str(), None), |(path, fragment)| {
                    (path, Some(fragment))
                });
            let path = decode_path(raw_path).unwrap_or_else(|error| {
                panic!("invalid percent-encoded path in {target:?}: {error}")
            });
            if path.is_empty() {
                if let Some(fragment) = raw_fragment.filter(|fragment| !fragment.is_empty()) {
                    assert_anchor_exists(&document, fragment, &target);
                }
                continue;
            }
            assert!(
                !Path::new(&path).is_absolute(),
                "{relative_doc} has an absolute link {target:?}"
            );
            let resolved = parent.join(&path);
            let canonical = resolved.canonicalize().unwrap_or_else(|error| {
                panic!("{relative_doc} has unresolved link {target:?}: {error}")
            });
            assert!(
                canonical.starts_with(&canonical_root),
                "{relative_doc} escapes repository root through {target:?}"
            );
            assert!(
                canonical.is_file(),
                "{relative_doc} link target is not a file: {target:?}"
            );
            if let Some(fragment) = raw_fragment.filter(|fragment| !fragment.is_empty()) {
                let linked_document = fs::read_to_string(&canonical).unwrap_or_else(|error| {
                    panic!("{relative_doc} link target {target:?} is not readable: {error}")
                });
                assert_anchor_exists(&linked_document, fragment, &target);
            }
        }
    }
}

#[test]
fn generated_reference_matches_clap_inventory_byte_for_byte() {
    let reference = normalize_generated_text(REFERENCE);
    assert_eq!(reference, render_cli_reference());
    assert!(reference.starts_with("<!-- BEGIN GENERATED CLI REFERENCE -->\n"));
    assert!(reference.ends_with("<!-- END GENERATED CLI REFERENCE -->\n"));
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
fn generated_cli_http_parity_is_fresh_and_anchored() {
    let manifest = checked_manifest().expect("parse checked CLI/HTTP parity manifest");
    check_docs_freshness(&manifest, PARITY).expect("CLI/HTTP parity documentation is stale");
}
#[test]
fn public_docs_expose_all_audience_tracks() {
    for heading in [
        "## New here",
        "## Operators",
        "## Integrators/AI agents",
        "## Contributors/maintainers",
    ] {
        assert!(
            DOCS_INDEX.contains(heading),
            "docs/README.md lacks audience track {heading:?}"
        );
    }
    let reference = docs_section(DOCS_INDEX, "## Referência");
    for target in REFERENCE_TARGETS {
        assert!(
            reference.contains(&format!("({target})")),
            "Referência section lacks canonical target {target}"
        );
    }
}

#[test]
fn changed_public_docs_have_resolved_repository_links() {
    assert_repository_relative_links_are_resolved();
}

#[test]
fn current_docs_quarantine_deleted_roadmap_and_temporal_promises() {
    let root = repo_root();
    assert!(
        !root.join("docs/control-roadmap.md").exists(),
        "deleted control roadmap must not remain in the current docs tree"
    );
    for (document, text) in [
        ("README.md", read_document(&root, "README.md")),
        ("docs/README.md", DOCS_INDEX.to_string()),
    ] {
        let lower = text.to_ascii_lowercase();
        for phrase in ["control-roadmap.md", "## roadmap", "## product direction"] {
            assert!(
                !lower.contains(phrase),
                "{document} retains deleted roadmap wording {phrase:?}"
            );
        }
    }

    let mut documents = Vec::new();
    markdown_files_under(&root.join("docs"), &mut documents);
    documents.push(root.join("README.md"));
    for path in documents {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read current documentation {path:?}: {error}"));
        let lower = text.to_ascii_lowercase();
        for phrase in AUDITED_TEMPORAL_PHRASES {
            assert!(
                !lower.contains(phrase),
                "current documentation {path:?} retains audited stale phrase {phrase:?}"
            );
        }
    }
}

fn read_document(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

#[test]
fn fleet_commands_are_owned_by_the_fleet_manual() {
    for (heading, command, flag) in [
        ("## Initialize a node", "omakure node init", "--scripts-dir"),
        (
            "## Discover peers",
            "omakure node discovery",
            "--wait-seconds",
        ),
        (
            "## Probe a trusted peer",
            "omakure node direct-probe",
            "--peer-node-id",
        ),
        (
            "## Trust a peer and set capabilities",
            "omakure node trust",
            "--confirmed",
        ),
        (
            "## Trust a peer and set capabilities",
            "omakure node capabilities",
            "--capability",
        ),
        ("## Remote Cues", "omakure node cue", "--script"),
        (
            "## Manual enrollment",
            "omakure node enroll request",
            "--role",
        ),
        (
            "## Manual enrollment",
            "omakure node enroll approve",
            "--request",
        ),
        (
            "## Manual enrollment",
            "omakure node enroll reject",
            "--confirmed",
        ),
        (
            "## Enrollment authority",
            "omakure node authority create",
            "--confirmed",
        ),
        (
            "## Enrollment authority",
            "omakure node authority show",
            "omakure node authority show",
        ),
        (
            "## Enrollment authority",
            "omakure node authority issue",
            "--audience",
        ),
        ("## Fleet health", "omakure node health", "--json"),
        (
            "## Baselines",
            "omakure node baseline create-key",
            "omakure node baseline create-key",
        ),
        ("## Baselines", "omakure node baseline publish", "--script"),
        ("## Baselines", "omakure node baseline push", "--manifest"),
        (
            "## Putting a machine back",
            "omakure node baseline rollback",
            "--confirmed",
        ),
        ("## Lifecycle Signals", "omakure node signals", "--json"),
    ] {
        let body = docs_section(FLEET, heading);
        assert!(body.contains(command), "{heading} lacks command {command}");
        assert!(body.contains(flag), "{heading} lacks flag {flag}");
    }
}

fn assert_operational_contract(
    document: &str,
    section: &str,
    commands: &[&str],
    flags: &[&str],
    requires_synopsis: bool,
) {
    let body = docs_section(document, section);
    if requires_synopsis {
        assert!(body.contains("**Synopsis:**"), "{section} lacks synopsis");
    }
    assert!(
        body.contains("```bash"),
        "{section} lacks executable example"
    );
    for command in commands {
        assert!(body.contains(command), "{section} lacks command {command}");
    }
    for flag in flags {
        assert!(body.contains(flag), "{section} lacks relevant flag {flag}");
    }
    let lower = body.to_ascii_lowercase();
    assert!(
        lower.contains("principal failure")
            || lower.contains("refus")
            || lower.contains("reject")
            || lower.contains("unreachable")
            || lower.contains("forbidden"),
        "{section} lacks a principal failure mode"
    );
    assert!(
        lower.contains("generated cli") && lower.contains("reference"),
        "{section} lacks reference link"
    );
}

#[test]
fn usage_does_not_retain_fleet_command_ownership() {
    for command in [
        "## `omakure node init`",
        "## `omakure node discovery`",
        "## `omakure node direct-probe`",
        "## `omakure node trust`",
        "## `omakure node capabilities`",
        "## `omakure node cue`",
        "## `omakure node enroll",
        "## `omakure node authority",
        "## `omakure node health`",
        "## `omakure node baseline",
        "## `omakure node signals`",
    ] {
        assert!(
            !USAGE.contains(command),
            "fleet command section remains in docs/usage.md: {command}"
        );
    }
    for invocation in [
        "omakure node init",
        "omakure node discovery",
        "omakure node direct-probe",
        "omakure node trust",
        "omakure node capabilities",
        "omakure node cue",
        "omakure node enroll",
        "omakure node authority",
        "omakure node health",
        "omakure node baseline",
        "omakure node signals",
    ] {
        assert!(
            !USAGE.contains(invocation),
            "fleet invocation remains in docs/usage.md: {invocation}"
        );
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;
    #[test]
    fn fragment_anchors_include_headings_and_explicit_ids() {
        let document =
            "# Intro\n## How to Run\n<a id=\"custom-section\"></a>\n<a name='legacy-section'></a>\n";
        assert_anchor_exists(document, "how-to-run", "guide.md#how-to-run");
        assert_anchor_exists(document, "custom-section", "guide.md#custom-section");
        assert_anchor_exists(document, "legacy-section", "guide.md#legacy-section");
        assert!(document_anchors(document).contains(&"intro".to_string()));
    }

    #[test]
    fn percent_encoded_paths_decode_per_segment_without_hiding_traversal() {
        assert_eq!(decode_path("how%20to.md"), Ok("how to.md".to_string()));
        let decoded = decode_path("%2e%2e/%2e%2e/etc").expect("decode traversal fixture");
        assert_eq!(decoded, "../../etc");
        assert!(
            std::path::Path::new(&decoded)
                .components()
                .any(|component| component == std::path::Component::ParentDir),
            "encoded parent segments must remain visible to repository confinement"
        );
    }

    #[test]
    fn generic_uri_schemes_are_not_repository_paths() {
        for target in [
            "mailto:ops@example.test",
            "tel:+15551212",
            "urn:isbn:1",
            "data:text/plain,ok",
        ] {
            assert!(
                uri_scheme(target).is_some(),
                "{target} must be recognized as a URI"
            );
        }
        assert!(uri_scheme("docs/guide.md").is_none());
        assert!(uri_scheme("//example.test/guide.md").is_none());
    }

    #[test]
    fn angle_bracket_destinations_preserve_spaces() {
        assert_eq!(
            markdown_targets("[guide](<docs/my guide.md>)"),
            vec!["docs/my guide.md".to_string()]
        );
        assert_eq!(
            markdown_targets("[guide](docs/guide.md \"title\")"),
            vec!["docs/guide.md".to_string()]
        );
    }

    #[test]
    fn windows_drive_absolute_paths_are_not_external_uris() {
        assert!(is_windows_drive_absolute(r"C:\docs\guide.md"));
        assert!(is_windows_drive_absolute("D:/docs/guide.md"));
        assert!(!is_windows_drive_absolute("docs/guide.md"));
        assert!(uri_scheme(r"C:\docs\guide.md").is_some());
    }

    #[test]
    fn missing_fragment_anchors_are_rejected() {
        let document = "# Existing heading\n<a name=\"explicit\"></a>\n";
        assert!(!document_anchors(document).contains(&"removed-heading".to_string()));
        assert!(!document_anchors(document).contains(&"missing".to_string()));
        assert!(
            std::panic::catch_unwind(|| {
                assert_anchor_exists(document, "removed-heading", "guide.md#removed-heading")
            })
            .is_err(),
            "removed fragments must fail the link contract"
        );
    }
}

#[test]
fn moved_fleet_sections_have_required_operational_contract() {
    assert_operational_contract(
        FLEET,
        "## Initialize a node",
        &["omakure node init"],
        &["--scripts-dir"],
        true,
    );
    assert_operational_contract(
        FLEET,
        "## Discover peers",
        &["omakure node discovery"],
        &["--wait-seconds", "--include-addresses"],
        true,
    );
    assert_operational_contract(
        FLEET,
        "## Probe a trusted peer",
        &["omakure node direct-probe"],
        &["--endpoint", "--peer-node-id"],
        true,
    );
    // Trust and capabilities share one narrative section and a concise
    // synopsis, while retaining command examples, flags, failures, and refs.
    assert_operational_contract(
        FLEET,
        "## Trust a peer and set capabilities",
        &["omakure node trust", "omakure node capabilities"],
        &["--public-key", "--capability", "--confirmed"],
        true,
    );
}

#[test]
fn usage_sections_have_required_operational_contract() {
    assert_operational_contract(
        USAGE,
        "## `omakure completion`",
        &["omakure completion bash"],
        &["SHELL"],
        true,
    );
}
