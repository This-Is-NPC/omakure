use crate::cli::args::ScriptsArgs;
use crate::cli::json;
use crate::operations::core::{self, ListScriptsRequest};
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

/// JSON shape for one script in `omakure scripts --json`.
///
/// This is the same shape used by `omakure search --json` so an agent can
/// pipe results between the two commands without translating fields.
pub type ScriptListEntry = core::ScriptSummary;

pub fn run(
    scripts_dir: PathBuf,
    args: ScriptsArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir.clone());
    let entries = core::list_scripts(&workspace, ListScriptsRequest { tags: args.tag })?;

    if json_output {
        json::print_ok(entries);
        return Ok(());
    }

    println!("Scripts folder: {}", scripts_dir.display());
    if entries.is_empty() {
        println!("(no scripts found)");
        return Ok(());
    }

    for entry in entries {
        let display = if entry.relative_path.is_empty() {
            entry.absolute_path.clone()
        } else {
            entry.relative_path.clone()
        };
        println!(" - {}", display);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_script(dir: &std::path::Path, name: &str, schema: Option<&str>) {
        let body = match schema {
            Some(s) => format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {}\n# OMAKURE_SCHEMA_END\necho hi\n",
                s
            ),
            None => "#!/usr/bin/env bash\necho hi\n".to_string(),
        };
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn run_human_format_with_scripts() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_script(
            tmp.path(),
            "deploy.sh",
            Some(r#"{"Name":"Deploy","Tags":["ops"],"Fields":[]}"#),
        );
        write_script(tmp.path(), "bare.sh", None);
        run(tmp.path().to_path_buf(), ScriptsArgs { tag: vec![] }, false).unwrap();
    }

    #[test]
    fn run_json_format_with_tag_filter() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_script(
            tmp.path(),
            "deploy.sh",
            Some(r#"{"Name":"Deploy","Tags":["ops"],"Fields":[]}"#),
        );
        write_script(
            tmp.path(),
            "noise.sh",
            Some(r#"{"Name":"Noise","Tags":["other"],"Fields":[]}"#),
        );
        run(
            tmp.path().to_path_buf(),
            ScriptsArgs {
                tag: vec!["ops".into()],
            },
            true,
        )
        .unwrap();
    }

    #[test]
    fn run_human_format_no_scripts() {
        let tmp = tempfile::TempDir::new().unwrap();
        run(tmp.path().to_path_buf(), ScriptsArgs { tag: vec![] }, false).unwrap();
    }
}
