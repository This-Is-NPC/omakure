//! `omakure describe <script>` — print the full schema of one script.

use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::cli::args::DescribeArgs;
use crate::cli::json::{self, codes};
use crate::cli::run::resolve_script_path;
use crate::domain::Schema;
use crate::error::{AppError, SchemaError};
use crate::ports::ScriptRepository;
use crate::workspace::Workspace;
use serde::Serialize;
use serde_json::json;
use std::error::Error;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct DescribePayload {
    pub absolute_path: String,
    pub relative_path: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub fields: Vec<DescribeField>,
}

#[derive(Debug, Serialize)]
pub struct DescribeField {
    pub name: String,
    pub prompt: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub order: u32,
    pub required: bool,
    pub arg: Option<String>,
    pub default: Option<String>,
    pub choices: Option<Vec<String>>,
}

pub fn run(
    scripts_dir: PathBuf,
    options: DescribeArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    let resolved = match resolve_script_path(&options.script, workspace.root()) {
        Ok(path) => path,
        Err(err) => {
            return emit_error(json_output, codes::NOT_FOUND, err.to_string());
        }
    };

    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let schema = match repo.read_schema(&resolved) {
        Ok(schema) => schema,
        Err(AppError::Schema(schema_err)) => {
            let code = match schema_err {
                SchemaError::BlockNotFound | SchemaError::JsonNotFound => codes::NOT_FOUND,
                _ => codes::SCHEMA_INVALID,
            };
            return emit_error(json_output, code, schema_err.to_string());
        }
        Err(err) => {
            return emit_error(json_output, codes::INTERNAL, err.to_string());
        }
    };

    if json_output {
        let payload = build_payload(&resolved, workspace.root(), &schema);
        json::print_ok(payload);
        return Ok(());
    }

    print_human(&resolved, workspace.root(), &schema);
    Ok(())
}

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message.clone());
        // Returning Err propagates a non-zero exit code, but main.rs's
        // top-level handler would print "error: <msg>" to stderr — which we
        // do not want when --json is set. Use process::exit directly.
        std::process::exit(1);
    }
    Err(message.into())
}

pub(crate) fn build_payload(script_path: &Path, root: &Path, schema: &Schema) -> DescribePayload {
    let mut fields: Vec<DescribeField> = schema
        .fields
        .iter()
        .map(|f| DescribeField {
            name: f.name.clone(),
            prompt: f.prompt.clone(),
            kind: f.kind.clone(),
            order: f.order,
            required: f.required.unwrap_or(false),
            arg: f.arg.clone(),
            default: f.default.clone(),
            choices: f.choices.clone(),
        })
        .collect();
    fields.sort_by_key(|f| f.order);

    let absolute_path = std::fs::canonicalize(script_path)
        .unwrap_or_else(|_| script_path.to_path_buf())
        .to_string_lossy()
        .to_string();
    let relative_path = script_path
        .strip_prefix(root)
        .unwrap_or(script_path)
        .to_string_lossy()
        .to_string();

    DescribePayload {
        absolute_path,
        relative_path,
        name: schema.name.clone(),
        description: schema.description.clone(),
        tags: schema.tags.clone().unwrap_or_default(),
        fields,
    }
}

fn print_human(script_path: &Path, root: &Path, schema: &Schema) {
    let payload = build_payload(script_path, root, schema);
    println!("Script: {}", payload.absolute_path);
    println!("Name: {}", payload.name);
    if let Some(desc) = &payload.description {
        println!("Description: {}", desc);
    }
    if !payload.tags.is_empty() {
        println!("Tags: {}", payload.tags.join(", "));
    }
    if payload.fields.is_empty() {
        println!("Fields: (none)");
        return;
    }
    println!("Fields:");
    for field in &payload.fields {
        let required = if field.required { " (required)" } else { "" };
        let arg = field.arg.as_deref().unwrap_or("");
        println!(
            "  - {} [{}]{}{}",
            field.name,
            field.kind,
            if arg.is_empty() {
                String::new()
            } else {
                format!(" {}", arg)
            },
            required
        );
        if let Some(prompt) = &field.prompt {
            println!("      prompt: {}", prompt);
        }
        if let Some(default) = &field.default {
            println!("      default: {}", default);
        }
        if let Some(choices) = &field.choices {
            println!("      choices: {}", choices.join(", "));
        }
    }
}

/// Render a sample envelope shape for `omakure help-ai`. Builds a fake
/// payload so the JSON example does not depend on a real workspace.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Field, Schema};
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn test_schema() -> Schema {
        Schema {
            name: "Deploy".to_string(),
            description: Some("Deploy the app".to_string()),
            tags: Some(vec!["ops".to_string()]),
            fields: vec![Field {
                name: "target".to_string(),
                prompt: Some("Target env".to_string()),
                kind: "string".to_string(),
                order: 1,
                required: Some(true),
                default: None,
                choices: Some(vec!["dev".to_string(), "prod".to_string()]),
                arg: Some("--target".to_string()),
            }],
            outputs: None,
            queue: None,
        }
    }

    #[test]
    fn test_build_payload_structure() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("deploy.sh");
        std::fs::write(&script, "#!/bin/bash").unwrap();

        let payload = build_payload(&script, tmp.path(), &test_schema());
        assert_eq!(payload.name, "Deploy");
        assert_eq!(payload.description, Some("Deploy the app".to_string()));
        assert_eq!(payload.tags, vec!["ops"]);
        assert_eq!(payload.fields.len(), 1);
        assert_eq!(payload.fields[0].name, "target");
        assert!(payload.fields[0].required);
        assert_eq!(
            payload.fields[0].choices,
            Some(vec!["dev".to_string(), "prod".to_string()])
        );
        assert_eq!(payload.relative_path, "deploy.sh");
    }

    #[test]
    fn test_build_payload_no_fields() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("simple.sh");
        std::fs::write(&script, "#!/bin/bash").unwrap();

        let schema = Schema {
            name: "Simple".to_string(),
            description: None,
            tags: None,
            fields: vec![],
            outputs: None,
            queue: None,
        };
        let payload = build_payload(&script, tmp.path(), &schema);
        assert_eq!(payload.name, "Simple");
        assert!(payload.description.is_none());
        assert!(payload.tags.is_empty());
        assert!(payload.fields.is_empty());
    }

    fn write_schema_script(tmp: &TempDir, name: &str, body: &str) -> PathBuf {
        let p = tmp.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn run_human_format_with_full_schema() {
        let tmp = TempDir::new().unwrap();
        write_schema_script(
            &tmp,
            "deploy.sh",
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\":\"Deploy\",\"Description\":\"Ship\",\"Tags\":[\"ops\"],\"Fields\":[{\"Name\":\"target\",\"Type\":\"string\",\"Order\":1,\"Required\":true,\"Arg\":\"--target\",\"Default\":\"prod\",\"Choices\":[\"dev\",\"prod\"],\"Prompt\":\"Target\"}]}\n# OMAKURE_SCHEMA_END\n",
        );
        run(
            tmp.path().to_path_buf(),
            DescribeArgs { script: "deploy.sh".into() },
            false,
        )
        .unwrap();
    }

    #[test]
    fn run_json_format_succeeds() {
        let tmp = TempDir::new().unwrap();
        write_schema_script(
            &tmp,
            "deploy.sh",
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\":\"Deploy\",\"Fields\":[]}\n# OMAKURE_SCHEMA_END\n",
        );
        run(
            tmp.path().to_path_buf(),
            DescribeArgs { script: "deploy.sh".into() },
            true,
        )
        .unwrap();
    }

    #[test]
    fn run_returns_not_found_for_missing_script() {
        let tmp = TempDir::new().unwrap();
        let err = run(
            tmp.path().to_path_buf(),
            DescribeArgs {
                script: "ghost.sh".into(),
            },
            false,
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn run_returns_not_found_when_script_lacks_schema() {
        let tmp = TempDir::new().unwrap();
        write_schema_script(&tmp, "bare.sh", "#!/usr/bin/env bash\necho hi\n");
        let err = run(
            tmp.path().to_path_buf(),
            DescribeArgs { script: "bare.sh".into() },
            false,
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn test_sample_envelope_shape() {
        let envelope = sample_envelope();
        assert_eq!(envelope["ok"], true);
        assert!(envelope["data"]["name"].is_string());
        assert!(envelope["data"]["fields"].is_array());
        assert_eq!(envelope["schema_version"], "1");
    }
}

pub fn sample_envelope() -> serde_json::Value {
    json::ok_envelope(json!({
        "absolute_path": "/abs/scripts/deploy.sh",
        "relative_path": "deploy.sh",
        "name": "deploy",
        "description": "Deploy the service",
        "tags": ["ops"],
        "fields": [
            {
                "name": "target",
                "prompt": "Target environment",
                "type": "string",
                "order": 1,
                "required": true,
                "arg": "--target",
                "default": null,
                "choices": ["dev", "prod"]
            }
        ]
    }))
}
