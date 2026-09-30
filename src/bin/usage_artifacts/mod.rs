//! Artifact helpers shared by the `usage-kdl` and `usage-docs` generators.

use std::fs;
use std::path::Path;

/// LF line endings, no trailing whitespace, and exactly one final newline, so
/// generated artifacts do not depend on the host that rendered them.
pub fn normalize(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut normalized = normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    normalized.push('\n');
    normalized
}

/// Fail unless `path` holds exactly `expected`, naming the `regenerate`
/// command that refreshes it.
pub fn compare_file(path: &str, expected: &str, regenerate: &str) -> Result<(), String> {
    let actual =
        fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    if actual != expected {
        return Err(format!("{path} is stale; run `{regenerate}`"));
    }
    Ok(())
}

pub fn write_file(path: &str, contents: &str) -> Result<(), String> {
    let parent = Path::new(path)
        .parent()
        .ok_or_else(|| format!("{path} has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    fs::write(path, contents).map_err(|error| format!("cannot write {path}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_removes_host_dependent_line_endings() {
        assert_eq!(normalize("a  \r\nb\r"), "a\nb\n");
        assert_eq!(normalize("a\nb\n"), "a\nb\n");
    }

    #[test]
    fn stale_file_comparison_fails_closed_and_names_the_fix() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        fs::write(&path, "stale\n").unwrap();
        let path = path.to_str().unwrap();

        assert_eq!(
            compare_file(path, "fresh\n", "regen"),
            Err(format!("{path} is stale; run `regen`"))
        );
        assert_eq!(compare_file(path, "stale\n", "regen"), Ok(()));
    }
}
