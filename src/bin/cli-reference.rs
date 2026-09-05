//! Generate and check the deterministic Clap-derived CLI reference.

use omakure::cli::inventory::{normalize_generated_text, render_cli_reference};
use std::env;
use std::fs;
use std::path::Path;

const REFERENCE: &str = "docs/cli-reference.md";
const START: &str = "<!-- BEGIN GENERATED CLI REFERENCE -->";
const END: &str = "<!-- END GENERATED CLI REFERENCE -->";

fn main() {
    let check = env::args().skip(1).any(|arg| arg == "--check");
    let generated = render_cli_reference();
    let path = Path::new(REFERENCE);
    let current = fs::read_to_string(path).ok();
    let expected = match current.as_deref() {
        Some(existing) => match replace_marked_region(existing, &generated) {
            Some(replaced) => replaced,
            None => {
                eprintln!("{REFERENCE} is missing complete generated markers");
                std::process::exit(1);
            }
        },
        None => generated.clone(),
    };

    if check {
        let current_normalized = current
            .as_deref()
            .map(normalize_generated_text)
            .unwrap_or_default();
        let expected_normalized = normalize_generated_text(&expected);
        if current_normalized != expected_normalized {
            eprintln!("{REFERENCE} is stale; run `cargo run --bin cli-reference`");
            std::process::exit(1);
        }
        return;
    }

    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("cannot create {}: {error}", parent.display());
            std::process::exit(1);
        }
    }
    if let Err(error) = fs::write(path, expected) {
        eprintln!("cannot write {REFERENCE}: {error}");
        std::process::exit(1);
    }
}

fn replace_marked_region(existing: &str, generated: &str) -> Option<String> {
    if existing.matches(START).count() != 1 || existing.matches(END).count() != 1 {
        return None;
    }
    let start = existing.find(START)?;
    let end = existing.find(END)?;
    if start >= end {
        return None;
    }
    let end = end + END.len();
    let mut output = String::with_capacity(existing.len() + generated.len());
    output.push_str(&existing[..start]);
    output.push_str(generated.trim_end_matches('\n'));
    output.push_str(&existing[end..]);
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marked_region_replacement_preserves_unmarked_content() {
        let existing = "prefix\n<!-- BEGIN GENERATED CLI REFERENCE -->\nold\n<!-- END GENERATED CLI REFERENCE -->\nsuffix\n";
        let generated =
            "<!-- BEGIN GENERATED CLI REFERENCE -->\nnew\n<!-- END GENERATED CLI REFERENCE -->\n";
        assert_eq!(
            replace_marked_region(existing, generated).unwrap(),
            "prefix\n<!-- BEGIN GENERATED CLI REFERENCE -->\nnew\n<!-- END GENERATED CLI REFERENCE -->\nsuffix\n"
        );
    }
    #[test]
    fn malformed_markers_are_rejected_without_replacing_content() {
        let generated =
            "<!-- BEGIN GENERATED CLI REFERENCE -->\nnew\n<!-- END GENERATED CLI REFERENCE -->\n";

        assert_eq!(replace_marked_region("old", generated), None);
        assert_eq!(
            replace_marked_region("<!-- BEGIN GENERATED CLI REFERENCE -->", generated),
            None
        );
    }

    #[test]
    fn ambiguous_markers_are_rejected_without_touching_content() {
        let generated =
            "<!-- BEGIN GENERATED CLI REFERENCE -->\nnew\n<!-- END GENERATED CLI REFERENCE -->\n";
        let cases = [
            "prefix <!-- BEGIN GENERATED CLI REFERENCE -->\n<!-- BEGIN GENERATED CLI REFERENCE -->\nold\n<!-- END GENERATED CLI REFERENCE -->",
            "<!-- END GENERATED CLI REFERENCE -->\n<!-- BEGIN GENERATED CLI REFERENCE -->\nold",
            "<!-- BEGIN GENERATED CLI REFERENCE -->\nold\n<!-- END GENERATED CLI REFERENCE -->\n<!-- END GENERATED CLI REFERENCE -->",
        ];
        for existing in cases {
            assert_eq!(replace_marked_region(existing, generated), None);
        }
    }
}
