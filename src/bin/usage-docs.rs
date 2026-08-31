//! Generate deterministic Markdown and roff documentation from the checked Usage KDL.
//!
//! Clap remains authoritative for parsing, help, and completions. This binary consumes the
//! checked-in Usage document and delegates rendering to the pinned Usage APIs.

use omakure::cli_http_parity::current_cli_ids;
use std::{collections::BTreeSet, env, fs};
use usage_docs::{
    docs::{manpage::ManpageRenderer, markdown::MarkdownRenderer},
    spec::cmd::SpecExample,
    Spec, SpecAdmonition, SpecArg, SpecChoice, SpecCommand, SpecFlag,
};

const KDL_PATH: &str = "docs/usage/omakure.kdl";
const MARKDOWN_PATH: &str = "docs/usage/omakure.md";
const MANPAGE_PATH: &str = "docs/usage/omakure.1";

fn replace_newline_placeholder(value: &mut String) {
    *value = value.replace("{n}", "\n");
}

fn normalize_option(value: &mut Option<String>) {
    if let Some(value) = value {
        replace_newline_placeholder(value);
    }
}

fn normalize_admonition(admonition: &mut SpecAdmonition) {
    replace_newline_placeholder(&mut admonition.text);
}

fn normalize_example(example: &mut SpecExample) {
    normalize_option(&mut example.header);
    normalize_option(&mut example.help);
}

fn normalize_choice(choice: &mut SpecChoice) {
    normalize_option(&mut choice.help);
}

fn normalize_arg(arg: &mut SpecArg) {
    normalize_option(&mut arg.help);
    normalize_option(&mut arg.help_long);
    normalize_option(&mut arg.help_md);
    normalize_option(&mut arg.help_first_line);
    for admonition in &mut arg.admonitions {
        normalize_admonition(admonition);
    }
    if let Some(choices) = &mut arg.choices {
        for choice in &mut choices.details {
            normalize_choice(choice);
        }
    }
}

fn normalize_flag(flag: &mut SpecFlag) {
    normalize_option(&mut flag.help);
    normalize_option(&mut flag.help_long);
    normalize_option(&mut flag.help_md);
    normalize_option(&mut flag.help_first_line);
    normalize_option(&mut flag.deprecated);
    for admonition in &mut flag.admonitions {
        normalize_admonition(admonition);
    }
    if let Some(arg) = &mut flag.arg {
        normalize_arg(arg);
    }
}

fn normalize_command(command: &mut SpecCommand) {
    normalize_option(&mut command.help);
    normalize_option(&mut command.help_long);
    normalize_option(&mut command.help_md);
    normalize_option(&mut command.before_help);
    normalize_option(&mut command.before_help_long);
    normalize_option(&mut command.before_help_md);
    normalize_option(&mut command.after_help);
    normalize_option(&mut command.after_help_long);
    normalize_option(&mut command.after_help_md);
    normalize_option(&mut command.deprecated);
    for example in &mut command.examples {
        normalize_example(example);
    }
    for heading in &mut command.headings {
        replace_newline_placeholder(&mut heading.help);
    }
    for output in &mut command.outputs {
        normalize_option(&mut output.help);
    }
    for exit_code in &mut command.exit_codes {
        replace_newline_placeholder(&mut exit_code.help);
    }
    for arg in &mut command.args {
        normalize_arg(arg);
    }
    for flag in &mut command.flags {
        normalize_flag(flag);
    }
    for child in command.subcommands.values_mut() {
        normalize_command(child);
    }
}

fn normalize_presentation(spec: &mut Spec) {
    normalize_option(&mut spec.about);
    normalize_option(&mut spec.about_long);
    normalize_option(&mut spec.about_md);
    normalize_option(&mut spec.before_help);
    normalize_option(&mut spec.after_help);
    normalize_option(&mut spec.before_help_long);
    normalize_option(&mut spec.after_help_long);
    for example in &mut spec.examples {
        normalize_example(example);
    }
    for output in &mut spec.outputs {
        normalize_option(&mut output.help);
    }
    for exit_code in &mut spec.exit_codes {
        replace_newline_placeholder(&mut exit_code.help);
    }
    normalize_command(&mut spec.cmd);
}

fn leaf_paths(command: &usage_docs::SpecCommand, paths: &mut Vec<String>) {
    if command.subcommands.is_empty() {
        paths.push(command.full_cmd.join(" "));
        return;
    }
    for child in command.subcommands.values() {
        leaf_paths(child, paths);
    }
}

fn validate_leaf_coverage(spec: &Spec) -> Result<Vec<String>, String> {
    let mut leaves = Vec::new();
    leaf_paths(&spec.cmd, &mut leaves);
    let actual = leaves.iter().cloned().collect::<BTreeSet<_>>();
    let expected = current_cli_ids().into_iter().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "checked Usage leaves do not match canonical CLI leaves (expected {}, got {})",
            expected.len(),
            actual.len()
        ));
    }
    Ok(leaves)
}

fn main() {
    match env::args().nth(1).as_deref() {
        None | Some("--write") => write_artifacts(),
        Some("--check") => check_artifacts(),
        Some(other) => {
            eprintln!("usage: usage-docs [--write|--check] (got {other})");
            std::process::exit(2);
        }
    }
}

fn render_artifacts() -> Result<(String, String), String> {
    let source =
        fs::read_to_string(KDL_PATH).map_err(|error| format!("cannot read {KDL_PATH}: {error}"))?;
    let mut spec: Spec = source
        .parse()
        .map_err(|error| format!("cannot parse {KDL_PATH}: {error}"))?;
    normalize_presentation(&mut spec);
    validate_leaf_coverage(&spec)?;
    let markdown = MarkdownRenderer::new(spec.clone())
        .with_replace_pre_with_code_fences(true)
        .render_spec()
        .map_err(|error| format!("cannot render Markdown: {error}"))?;
    let manpage = ManpageRenderer::new(spec)
        .render()
        .map_err(|error| format!("cannot render manpage: {error}"))?;
    Ok((normalize(markdown), normalize(manpage)))
}

fn write_artifacts() {
    let (markdown, manpage) = render_artifacts().unwrap_or_else(|error| fail("write", error));
    write_file(MARKDOWN_PATH, &markdown).unwrap_or_else(|error| fail("write", error));
    write_file(MANPAGE_PATH, &manpage).unwrap_or_else(|error| fail("write", error));
}

fn check_artifacts() {
    let (markdown, manpage) = render_artifacts().unwrap_or_else(|error| fail("check", error));
    compare_file(MARKDOWN_PATH, &markdown).unwrap_or_else(|error| fail("check", error));
    compare_file(MANPAGE_PATH, &manpage).unwrap_or_else(|error| fail("check", error));
}

fn normalize(value: String) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    lines.push('\n');
    lines
}

fn compare_file(path: &str, expected: &str) -> Result<(), String> {
    let actual =
        fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    if actual != expected {
        return Err(format!(
            "{path} is stale; run `scripts/tasks/usage-docs --write`"
        ));
    }
    Ok(())
}

fn write_file(path: &str, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|error| format!("cannot write {path}: {error}"))
}

fn fail(operation: &str, error: String) -> ! {
    eprintln!("usage-docs {operation} failed: {error}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_is_deterministic_and_covers_all_canonical_leaves() {
        let source = fs::read_to_string(KDL_PATH).unwrap();
        let spec: Spec = source.parse().unwrap();
        let mut leaves = Vec::new();
        leaf_paths(&spec.cmd, &mut leaves);
        assert_eq!(leaves.len(), 65);
        let first = render_artifacts().unwrap();
        let second = render_artifacts().unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read_to_string(MARKDOWN_PATH).unwrap(), first.0);
        assert_eq!(fs::read_to_string(MANPAGE_PATH).unwrap(), first.1);
        assert!(!first.0.contains("{n}"));
        assert!(!first.1.contains("{n}"));
        assert!(first.0.contains("# `omakure`"));
        assert!(first.1.starts_with(".TH OMAKURE 1"));
        for path in leaves {
            let heading = format!("## `{} {path}`", spec.bin);
            assert!(
                first.0.contains(&heading),
                "Markdown omits canonical leaf {path}"
            );
            let roff_path = path.replace('-', r"\-");
            assert!(
                first.1.contains(&format!(r"\fB{roff_path}\fR")),
                "manpage omits canonical leaf {path}"
            );
        }
    }

    #[test]
    fn presentation_normalization_replaces_only_the_known_marker() {
        let source = fs::read_to_string(KDL_PATH).unwrap();
        let mut spec: Spec = source.parse().unwrap();
        let original_name = spec.cmd.name.clone();
        assert!(spec.about_long.as_deref().unwrap().contains("{n}"));
        let original_json_shape = "{ ok, data, error, schema_version }";
        assert!(spec
            .about_long
            .as_deref()
            .unwrap()
            .contains(original_json_shape));
        normalize_presentation(&mut spec);
        assert!(spec.about_long.as_deref().unwrap().contains('\n'));
        assert!(!spec.about_long.as_deref().unwrap().contains("{n}"));
        assert!(spec
            .about_long
            .as_deref()
            .unwrap()
            .contains(original_json_shape));
        assert_eq!(spec.cmd.name, original_name);
    }

    #[test]
    fn stale_file_comparison_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        fs::write(&path, "stale\n").unwrap();
        assert!(compare_file(path.to_str().unwrap(), "fresh\n").is_err());
    }

    #[test]
    fn normalization_removes_host_dependent_line_endings() {
        assert_eq!(normalize("a  \r\nb\r".into()), "a\nb\n");
    }
}
