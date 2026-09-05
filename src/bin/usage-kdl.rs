//! Generate and check the Clap-derived Usage KDL compatibility artifact.
//!
//! Clap remains authoritative for parsing, help, and completions. This binary
//! is deliberately feature-gated: `clap_usage` is not part of the shipped
//! omakure dependency graph.

use clap::{CommandFactory, Parser};
use omakure::cli::args::Cli;
use omakure::cli_http_parity::{checked_manifest, current_cli_ids};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;

const KDL_PATH: &str = "docs/usage/omakure.kdl";
const EVIDENCE_PATH: &str = "docs/usage/fidelity.json";
const OVERLAY_PATH: &str = "docs/usage/overlay.json";
const ALLOWLIST_PATH: &str = "docs/usage/fidelity-allowlist.json";
const RESIDUAL_PATH: &str = "docs/usage/unreportable-semantics.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
struct ClapUsageIdentity {
    git: String,
    version: String,
    revision: String,
    commit: String,
}

fn clap_usage_identity() -> Result<ClapUsageIdentity, String> {
    let manifest = fs::read_to_string("Cargo.toml")
        .map_err(|error| format!("cannot read Cargo.toml: {error}"))?;
    let lock = fs::read_to_string("Cargo.lock")
        .map_err(|error| format!("cannot read Cargo.lock: {error}"))?;
    resolve_clap_usage_identity(&manifest, &lock)
}

fn resolve_clap_usage_identity(
    manifest_text: &str,
    lock_text: &str,
) -> Result<ClapUsageIdentity, String> {
    let manifest: toml::Value = toml::from_str(manifest_text)
        .map_err(|error| format!("cannot parse Cargo.toml: {error}"))?;
    let dependency = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .and_then(|dependencies| dependencies.get("clap_usage"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "Cargo.toml has no table dependency named clap_usage".to_string())?;
    let manifest_version = dependency
        .get("version")
        .and_then(toml::Value::as_str)
        .and_then(|version| version.strip_prefix('='))
        .ok_or_else(|| "clap_usage manifest version must be an exact = requirement".to_string())?;
    let manifest_git = dependency
        .get("git")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "clap_usage manifest dependency must pin git".to_string())?;
    let manifest_revision = dependency
        .get("rev")
        .and_then(toml::Value::as_str)
        .filter(|revision| !revision.is_empty())
        .ok_or_else(|| "clap_usage manifest dependency must pin rev".to_string())?;

    let lock: toml::Value =
        toml::from_str(lock_text).map_err(|error| format!("cannot parse Cargo.lock: {error}"))?;
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "Cargo.lock has no package array".to_string())?;
    let matches = packages
        .iter()
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some("clap_usage"))
        .collect::<Vec<_>>();
    let package = match matches.as_slice() {
        [package] => package,
        [] => return Err("Cargo.lock has no clap_usage package".to_string()),
        _ => return Err("Cargo.lock has multiple clap_usage packages".to_string()),
    };
    let lock_version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "clap_usage lock package has no version".to_string())?;
    let source = package
        .get("source")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "clap_usage lock package has no source".to_string())?;
    let source = source
        .strip_prefix("git+")
        .ok_or_else(|| "clap_usage lock source is not a git source".to_string())?;
    let (source_url, source_query) = source
        .split_once('?')
        .ok_or_else(|| "clap_usage lock source has no query".to_string())?;
    let (source_query, commit) = source_query
        .split_once('#')
        .ok_or_else(|| "clap_usage lock source has no resolved commit".to_string())?;
    let lock_revision = source_query
        .split('&')
        .find_map(|part| part.strip_prefix("rev="))
        .filter(|revision| !revision.is_empty())
        .ok_or_else(|| "clap_usage lock source has no rev query".to_string())?;
    if lock_version != manifest_version {
        return Err(format!(
            "clap_usage version drift: Cargo.toml {manifest_version}, Cargo.lock {lock_version}"
        ));
    }
    if source_url != manifest_git {
        return Err(format!(
            "clap_usage git drift: Cargo.toml {manifest_git}, Cargo.lock {source_url}"
        ));
    }
    if lock_revision != manifest_revision {
        return Err(format!(
            "clap_usage rev drift: Cargo.toml {manifest_revision}, Cargo.lock {lock_revision}"
        ));
    }
    if commit.is_empty() {
        return Err("clap_usage lock source has an empty resolved commit".to_string());
    }
    if !commit.starts_with(lock_revision) {
        return Err(format!(
            "clap_usage resolved commit {commit} does not match pinned rev {lock_revision}"
        ));
    }
    Ok(ClapUsageIdentity {
        git: manifest_git.to_string(),
        version: lock_version.to_string(),
        revision: lock_revision.to_string(),
        commit: commit.to_string(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
struct UnreportableSemantic {
    command: Vec<String>,
    argument: String,
    kind: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
struct LossEvidence {
    command: Vec<String>,
    argument: Option<String>,
    feature: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
struct FidelityEvidence {
    schema_version: u32,
    generator: String,
    clap_usage_git: String,
    clap_usage_version: String,
    clap_usage_revision: String,
    clap_usage_commit: String,
    command_count: usize,
    leaf_count: usize,
    losses: Vec<LossEvidence>,
    unreportable_semantics: Vec<UnreportableSemantic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
struct OverlayEntry {
    entry_id: String,
    operation_family: String,
    cli_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
struct Overlay {
    schema_version: u32,
    entries: Vec<OverlayEntry>,
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => {
            if let Err(error) = write_artifacts(&generate_artifacts()) {
                eprintln!("usage-kdl write failed: {error}");
                std::process::exit(1);
            }
        }
        [arg] if arg == "--write" => {
            if let Err(error) = write_artifacts(&generate_artifacts()) {
                eprintln!("usage-kdl write failed: {error}");
                std::process::exit(1);
            }
        }
        [arg] if arg == "--check" => {
            let (kdl, evidence, overlay, losses) = generate_artifacts();
            if let Err(error) = check_artifacts(&kdl, &evidence, &overlay, &losses) {
                eprintln!("usage-kdl check failed: {error}");
                std::process::exit(1);
            }
        }
        [arg] if arg == "--review" => {
            let (_, _, _, losses) = generate_artifacts();
            if let Err(error) = review_losses(&losses) {
                eprintln!("usage-kdl review failed: {error}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: usage-kdl [--write|--check|--review]");
            std::process::exit(2);
        }
    }
}

fn generate_artifacts() -> (String, FidelityEvidence, Overlay, Vec<LossEvidence>) {
    let identity = clap_usage_identity().unwrap_or_else(|error| {
        eprintln!("invalid clap_usage dependency identity: {error}");
        std::process::exit(1);
    });
    let mut command = Cli::command();
    let (spec, report) = clap_usage::spec_with_report(&mut command, "omakure");
    let kdl = normalize_kdl(&format!(
        "// @generated by usage-kdl from Clap metadata\n// clap_usage {} ({}, rev {}, commit {})\n\n{spec}",
        identity.version, identity.git, identity.revision, identity.commit
    ));

    let losses = report
        .losses()
        .iter()
        .map(|loss| LossEvidence {
            command: loss.command.clone(),
            argument: loss.argument.clone(),
            feature: format!("{:?}", loss.feature),
            detail: loss.detail.clone(),
        })
        .collect::<Vec<_>>();
    let command_count = count_commands(&command);
    let leaf_count = current_cli_ids().len();
    let unreportable_semantics = collect_unreportable_semantics(&command);
    validate_unreportable_semantics(&command, &unreportable_semantics).unwrap_or_else(|error| {
        eprintln!("invalid residual semantics: {error}");
        std::process::exit(1);
    });
    let evidence = FidelityEvidence {
        schema_version: SCHEMA_VERSION,
        generator: "Cli::command() -> clap_usage::spec_with_report".to_string(),
        clap_usage_git: identity.git,
        clap_usage_version: identity.version,
        clap_usage_revision: identity.revision,
        clap_usage_commit: identity.commit,
        command_count,
        leaf_count,
        losses: losses.clone(),
        unreportable_semantics,
    };
    let manifest = checked_manifest().unwrap_or_else(|error| {
        eprintln!("cannot parse parity manifest: {error}");
        std::process::exit(1);
    });
    let mut entries = manifest
        .entries
        .into_iter()
        .filter(|entry| !entry.cli_ids.is_empty())
        .map(|entry| OverlayEntry {
            entry_id: entry.entry_id,
            operation_family: entry.operation_family,
            cli_ids: entry.cli_ids,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
    for entry in &mut entries {
        entry.cli_ids.sort();
    }
    let overlay = Overlay {
        schema_version: SCHEMA_VERSION,
        entries,
    };
    validate_overlay(&overlay).unwrap_or_else(|error| {
        eprintln!("invalid Usage overlay: {error}");
        std::process::exit(1);
    });
    (kdl, evidence, overlay, losses)
}

fn validate_overlay(overlay: &Overlay) -> Result<(), String> {
    if overlay.schema_version != SCHEMA_VERSION {
        return Err(format!("schema_version must be {SCHEMA_VERSION}"));
    }
    let mut mapped = BTreeSet::new();
    let mut entry_ids = BTreeSet::new();
    for entry in &overlay.entries {
        if entry.entry_id.trim().is_empty() || entry.operation_family.trim().is_empty() {
            return Err("entry_id and operation_family must be non-empty".to_string());
        }
        if !entry_ids.insert(entry.entry_id.as_str()) {
            return Err(format!("duplicate entry_id {}", entry.entry_id));
        }
        for cli_id in &entry.cli_ids {
            if !mapped.insert(cli_id.as_str()) {
                return Err(format!("duplicate CLI ID {cli_id}"));
            }
        }
    }
    let expected = current_cli_ids().into_iter().collect::<BTreeSet<_>>();
    let actual = mapped
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "overlay CLI IDs do not match Clap leaves (expected {}, got {})",
            expected.len(),
            actual.len()
        ));
    }
    Ok(())
}
fn count_commands(command: &clap::Command) -> usize {
    1 + command.get_subcommands().map(count_commands).sum::<usize>()
}
fn collect_unreportable_semantics(command: &clap::Command) -> Vec<UnreportableSemantic> {
    let mut semantics = Vec::new();
    collect_unreportable_for_command(command, &[], &mut semantics);
    // Clap's required_unless_present relation is setter-only: clap exposes no
    // getter for it, so keep the current declaration explicit and reviewed.
    semantics.push(UnreportableSemantic {
        command: vec!["omakure".into(), "init".into()],
        argument: "script".into(),
        kind: "required-unless-present".into(),
        detail: "required_unless_present=name".into(),
    });
    semantics.sort_by(|left, right| {
        (&left.command, &left.argument, &left.kind, &left.detail).cmp(&(
            &right.command,
            &right.argument,
            &right.kind,
            &right.detail,
        ))
    });
    semantics
}

fn collect_unreportable_for_command(
    command: &clap::Command,
    ancestors: &[String],
    semantics: &mut Vec<UnreportableSemantic>,
) {
    let mut path = ancestors.to_vec();
    path.push(command.get_name().to_string());
    for argument in command.get_arguments() {
        if let Some(env) = argument.get_env() {
            semantics.push(UnreportableSemantic {
                command: path.clone(),
                argument: argument.get_id().to_string(),
                kind: "environment-fallback".into(),
                detail: format!("env={}", env.to_string_lossy()),
            });
        }
    }
    for subcommand in command.get_subcommands() {
        collect_unreportable_for_command(subcommand, &path, semantics);
    }
}

fn validate_unreportable_semantics(
    command: &clap::Command,
    semantics: &[UnreportableSemantic],
) -> Result<(), String> {
    for semantic in semantics {
        let Some((root, path)) = semantic.command.split_first() else {
            return Err("residual semantic has an empty command path".to_string());
        };
        if root != command.get_name() {
            return Err(format!(
                "residual semantic root {} does not match {}",
                root,
                command.get_name()
            ));
        }
        let Some(target) = find_command(command, path) else {
            return Err(format!("missing command {}", semantic.command.join(" ")));
        };
        let Some(argument) = target
            .get_arguments()
            .find(|arg| arg.get_id().as_str() == semantic.argument)
        else {
            return Err(format!(
                "missing argument {} on {}",
                semantic.argument,
                semantic.command.join(" ")
            ));
        };
        match semantic.kind.as_str() {
            "environment-fallback" => {
                let expected = semantic.detail.strip_prefix("env=").ok_or_else(|| {
                    format!("malformed environment detail for {}", semantic.argument)
                })?;
                if argument
                    .get_env()
                    .is_none_or(|env| env.to_string_lossy() != expected)
                {
                    return Err(format!(
                        "environment metadata changed for {} {}",
                        semantic.command.join(" "),
                        semantic.argument
                    ));
                }
            }
            "required-unless-present" => {
                let fallback = semantic
                    .detail
                    .strip_prefix("required_unless_present=")
                    .filter(|fallback| !fallback.is_empty())
                    .ok_or_else(|| {
                        format!("malformed requiredness detail for {}", semantic.argument)
                    })?;
                let mut missing = semantic.command.clone();
                if Cli::try_parse_from(&missing).is_ok() {
                    return Err(format!(
                        "{} {} is no longer required without --{fallback}",
                        semantic.command.join(" "),
                        semantic.argument
                    ));
                }
                missing.push(format!("--{fallback}"));
                missing.push("example".to_string());
                if Cli::try_parse_from(&missing).is_err() {
                    return Err(format!(
                        "{} {} no longer accepts --{fallback} as its requiredness escape",
                        semantic.command.join(" "),
                        semantic.argument
                    ));
                }
            }
            kind => {
                return Err(format!("unsupported residual semantic kind {kind}"));
            }
        }
    }
    Ok(())
}

fn find_command<'a>(command: &'a clap::Command, path: &[String]) -> Option<&'a clap::Command> {
    if path.is_empty() {
        return Some(command);
    }
    let next = path.first()?;
    command
        .get_subcommands()
        .find(|subcommand| subcommand.get_name() == next)
        .and_then(|subcommand| find_command(subcommand, &path[1..]))
}
fn review_losses(losses: &[LossEvidence]) -> Result<(), String> {
    let reviewed: Vec<LossEvidence> = read_json(ALLOWLIST_PATH)?;
    if reviewed == losses {
        println!("No fidelity loss changes detected.");
        return Ok(());
    }
    println!("Detected fidelity loss changes; inspect and explicitly review them.");
    for loss in losses.iter().filter(|loss| !reviewed.contains(loss)) {
        let rendered = serde_json::to_string(loss)
            .map_err(|error| format!("cannot serialize detected loss: {error}"))?;
        println!("+ {rendered}");
    }
    for loss in reviewed.iter().filter(|loss| !losses.contains(loss)) {
        let rendered = serde_json::to_string(loss)
            .map_err(|error| format!("cannot serialize removed loss: {error}"))?;
        println!("- {rendered}");
    }
    let candidate = serde_json::to_string_pretty(losses)
        .map_err(|error| format!("cannot serialize candidate allowlist: {error}"))?;
    println!("Candidate {ALLOWLIST_PATH} (replace only after review):");
    println!("{candidate}");
    Ok(())
}

fn check_artifacts(
    kdl: &str,
    evidence: &FidelityEvidence,
    overlay: &Overlay,
    losses: &[LossEvidence],
) -> Result<(), String> {
    compare_file(KDL_PATH, kdl)?;
    compare_json(EVIDENCE_PATH, evidence)?;
    compare_json(OVERLAY_PATH, overlay)?;
    compare_json(RESIDUAL_PATH, &evidence.unreportable_semantics)?;
    compare_json(ALLOWLIST_PATH, &losses.to_vec())?;
    Ok(())
}
fn write_artifacts(
    artifacts: &(String, FidelityEvidence, Overlay, Vec<LossEvidence>),
) -> Result<(), String> {
    let (kdl, evidence, overlay, losses) = artifacts;
    let allowlist: Vec<LossEvidence> = read_json(ALLOWLIST_PATH)?;
    if allowlist != *losses {
        return Err(format!(
            "{ALLOWLIST_PATH} differs from detected losses; refusing to approve unreviewed fidelity changes"
        ));
    }
    let residuals: Vec<UnreportableSemantic> = read_json(RESIDUAL_PATH)?;
    if residuals != evidence.unreportable_semantics {
        return Err(format!(
            "{RESIDUAL_PATH} differs from current setter-only semantics; refusing unreviewed changes"
        ));
    }
    write_file(KDL_PATH, kdl)?;
    write_json(EVIDENCE_PATH, evidence)?;
    write_json(OVERLAY_PATH, overlay)?;
    write_json(ALLOWLIST_PATH, &allowlist)?;
    write_json(RESIDUAL_PATH, &residuals)?;
    Ok(())
}

fn normalize_kdl(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut normalized = normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    normalized.push('\n');
    normalized
}

fn compare_file(path: &str, expected: &str) -> Result<(), String> {
    let actual =
        fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    if actual != expected {
        return Err(format!(
            "{path} is stale; run `cargo run --features usage-generator --bin usage-kdl`"
        ));
    }
    Ok(())
}

fn compare_json<T: Serialize + for<'de> Deserialize<'de> + Eq>(
    path: &str,
    expected: &T,
) -> Result<(), String> {
    let actual: T = read_json(path)?;
    if &actual != expected {
        return Err(format!("{path} is stale; regenerate Usage artifacts"));
    }
    let expected_bytes = serde_json::to_vec_pretty(expected)
        .map_err(|error| format!("cannot serialize {path}: {error}"))?;
    let expected_text = format!("{}\n", String::from_utf8_lossy(&expected_bytes));
    let actual_text =
        fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    if actual_text != expected_text {
        return Err(format!("{path} is not deterministically formatted"));
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("cannot parse {path}: {error}"))
}

fn write_file(path: &str, contents: &str) -> Result<(), String> {
    let parent = Path::new(path)
        .parent()
        .ok_or_else(|| format!("{path} has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    fs::write(path, contents).map_err(|error| format!("cannot write {path}: {error}"))
}

fn write_json<T: Serialize>(path: &str, value: &T) -> Result<(), String> {
    let contents = serde_json::to_string_pretty(value)
        .map_err(|error| format!("cannot serialize {path}: {error}"))?;
    write_file(path, &format!("{contents}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kdl_normalization_is_deterministic() {
        assert_eq!(normalize_kdl("a  \r\nb\r"), "a\nb\n");
        assert_eq!(normalize_kdl("a\nb\n"), "a\nb\n");
    }

    #[test]
    fn overlay_has_no_usage_full_command_identity() {
        let json = serde_json::to_string(&Overlay {
            schema_version: 1,
            entries: vec![OverlayEntry {
                entry_id: "x".into(),
                operation_family: "y".into(),
                cli_ids: vec!["z".into()],
            }],
        })
        .unwrap();
        assert!(!json.contains("full_cmd"));
    }
    #[test]
    fn pinned_report_detects_supported_argument_losses() {
        let mut command = clap::Command::new("test")
            .arg(clap::Arg::new("env").long("env").env("TEST_ENV"))
            .arg(
                clap::Arg::new("many")
                    .long("many")
                    .action(clap::ArgAction::Set)
                    .num_args(0..=2)
                    .value_names(["FIRST", "SECOND"]),
            )
            .arg(
                clap::Arg::new("delimited")
                    .long("delimited")
                    .action(clap::ArgAction::Set)
                    .value_delimiter('·'),
            );
        let (_, report) = clap_usage::spec_with_report(&mut command, "test");
        let features = report
            .losses()
            .iter()
            .map(|loss| format!("{:?}", loss.feature))
            .collect::<BTreeSet<_>>();
        assert!(features.contains("Environment"));
        assert!(features.contains("ValueArity"));
        assert!(features.contains("DistinctValueNames"));
        assert!(features.contains("NonAsciiDelimiter"));
    }

    #[test]
    fn pinned_report_detects_command_rendering_losses() {
        let mut command = clap::Command::new("test")
            .disable_colored_help(true)
            .color(clap::ColorChoice::Always);
        let (_, report) = clap_usage::spec_with_report(&mut command, "test");
        let features = report
            .losses()
            .iter()
            .map(|loss| format!("{:?}", loss.feature))
            .collect::<BTreeSet<_>>();
        assert!(features.contains("DisableColoredHelp"));
        assert!(features.contains("Color"));
    }
    #[test]
    fn parity_overlay_covers_all_canonical_leaves() {
        let (_, evidence, overlay, _) = generate_artifacts();
        assert_eq!(evidence.leaf_count, 65);
        validate_overlay(&overlay).unwrap();
    }

    #[test]
    fn residual_init_condition_preserves_clap_parser_boundary() {
        use clap::Parser;

        assert!(Cli::try_parse_from(["omakure", "init"]).is_err());
        assert!(Cli::try_parse_from(["omakure", "init", "--name", "example"]).is_ok());
    }

    #[test]
    fn residual_requiredness_mutation_is_rejected() {
        let command = Cli::command();
        let semantic = UnreportableSemantic {
            command: vec!["omakure".into(), "init".into()],
            argument: "script".into(),
            kind: "required-unless-present".into(),
            detail: "required_unless_present=name".into(),
        };
        validate_unreportable_semantics(&command, std::slice::from_ref(&semantic)).unwrap();

        let mut mutated = semantic;
        mutated.detail = "required_unless_present=other".into();
        assert!(validate_unreportable_semantics(&command, &[mutated]).is_err());
    }

    #[test]
    fn dependency_identity_mutations_are_rejected() {
        let manifest = fs::read_to_string("Cargo.toml").unwrap();
        let lock = fs::read_to_string("Cargo.lock").unwrap();
        let identity = resolve_clap_usage_identity(&manifest, &lock).unwrap();
        assert!(!identity.commit.is_empty());

        let version_marker = format!("version = \"={}\"", identity.version);
        let mutated_manifest = manifest.replace(&version_marker, "version = \"=0.0.0\"");
        assert!(resolve_clap_usage_identity(&mutated_manifest, &lock).is_err());

        let revision_marker = format!("?rev={}#", identity.revision);
        let mutated_lock = lock.replace(&revision_marker, "?rev=drifted#");
        assert!(resolve_clap_usage_identity(&manifest, &mutated_lock).is_err());
        let commit_marker = format!("#{}", identity.commit);
        let mutated_commit_lock = lock.replace(&commit_marker, "#deadbeef");
        assert!(resolve_clap_usage_identity(&manifest, &mutated_commit_lock).is_err());
    }

    #[test]
    fn mutated_allowlist_and_artifact_are_detected() {
        let loss = LossEvidence {
            command: vec!["test".into()],
            argument: None,
            feature: "Color".into(),
            detail: "changed".into(),
        };
        let expected = vec![loss.clone()];
        let mut mutated = expected.clone();
        mutated[0].detail = "mutated".into();
        let dir = tempfile::tempdir().unwrap();
        let allowlist_path = dir.path().join("allowlist.json");
        fs::write(&allowlist_path, serde_json::to_vec(&mutated).unwrap()).unwrap();
        assert!(compare_json(allowlist_path.to_str().unwrap(), &expected).is_err());

        let artifact_path = dir.path().join("artifact.kdl");
        fs::write(&artifact_path, "stale\n").unwrap();
        assert!(compare_file(artifact_path.to_str().unwrap(), "fresh\n").is_err());
    }
}
