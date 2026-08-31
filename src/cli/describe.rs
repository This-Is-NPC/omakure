//! `omakure describe <script>` — print the full schema of one script.

use crate::cli::args::DescribeArgs;
use crate::cli::json::{self, codes};
use crate::operations::core::{self, DescribeScriptRequest, ScriptDescription};
use crate::operations::{OperationError, OperationErrorCode};
use crate::workspace::Workspace;
use serde::Serialize;
use serde_json::json;
use std::error::Error;
use std::path::PathBuf;

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
    let description = match core::describe_script(
        &workspace,
        DescribeScriptRequest {
            script: options.script,
        },
    ) {
        Ok(description) => description,
        Err(err) => return emit_operation_error(json_output, err),
    };

    if json_output {
        json::print_ok(payload_from_description(description));
        return Ok(());
    }

    print_human_payload(&payload_from_description(description));
    Ok(())
}

fn emit_operation_error(json_output: bool, err: OperationError) -> Result<(), Box<dyn Error>> {
    let code = match err.code {
        OperationErrorCode::NotFound => codes::NOT_FOUND,
        OperationErrorCode::InvalidInput if is_missing_schema_message(&err.message) => {
            codes::NOT_FOUND
        }
        OperationErrorCode::UnsafePath => codes::INVALID_ARGUMENT,
        OperationErrorCode::InvalidInput => codes::SCHEMA_INVALID,
        _ => codes::INTERNAL,
    };
    emit_error(json_output, code, err.message)
}

fn is_missing_schema_message(message: &str) -> bool {
    message.contains("Schema block not found") || message.contains("Schema JSON object not found")
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

#[cfg(test)]
pub(crate) fn build_payload(
    script_path: &std::path::Path,
    root: &std::path::Path,
    schema: &crate::domain::Schema,
) -> DescribePayload {
    let mut fields: Vec<DescribeField> = schema
        .fields
        .iter()
        .map(|f| DescribeField {
            name: f.name.clone(),
            prompt: f.prompt.clone(),
            kind: f.kind.clone(),
            order: f.order.unwrap_or(0),
            required: f.required.unwrap_or(false),
            arg: f.arg.clone(),
            default: if f.is_secret() {
                None
            } else {
                f.default.clone()
            },
            choices: f.choices.clone(),
        })
        .collect();
    fields.sort_by_key(|f| f.order);

    let absolute_path = std::fs::canonicalize(script_path)
        .unwrap_or_else(|_| script_path.to_path_buf())
        .to_string_lossy()
        .to_string();
    let relative_path = logical_relative_path(script_path, root);

    DescribePayload {
        absolute_path,
        relative_path,
        name: schema.name.clone(),
        description: schema.description.clone(),
        tags: schema.tags.clone().unwrap_or_default(),
        fields,
    }
}

fn payload_from_description(description: ScriptDescription) -> DescribePayload {
    DescribePayload {
        absolute_path: description.absolute_path,
        relative_path: description.relative_path,
        name: description.schema.name,
        description: description.schema.description,
        tags: description.schema.tags,
        fields: description
            .schema
            .fields
            .into_iter()
            .map(|field| DescribeField {
                name: field.name,
                prompt: field.prompt,
                kind: field.kind.clone(),
                order: field.order,
                required: field.required,
                arg: field.arg,
                default: if field.kind.eq_ignore_ascii_case("secret") {
                    None
                } else {
                    field.default
                },
                choices: field.choices,
            })
            .collect(),
    }
}

fn logical_relative_path(path: &std::path::Path, root: &std::path::Path) -> String {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path_text = canonical_path.to_string_lossy().replace('\\', "/");
    let root_text = canonical_root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    path_text
        .strip_prefix(&root_text)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(&path_text)
        .to_string()
}

fn print_human_payload(payload: &DescribePayload) {
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
                order: Some(1),
                required: Some(true),
                default: None,
                choices: Some(vec!["dev".to_string(), "prod".to_string()]),
                arg: Some("--target".to_string()),
            }],
            outputs: None,
            queue: None,
            schedule: None,
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
    fn build_payload_marks_secret_fields_without_returning_values() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("deploy.sh");
        std::fs::write(&script, "#!/bin/bash").unwrap();

        let mut schema = test_schema();
        schema.fields[0].kind = "secret".to_string();
        schema.fields[0].default = Some("supersecret".to_string());

        let payload = build_payload(&script, tmp.path(), &schema);
        assert_eq!(payload.fields[0].kind, "secret");
        assert_eq!(payload.fields[0].default, None);
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
            schedule: None,
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
            DescribeArgs {
                script: "deploy.sh".into(),
            },
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
            DescribeArgs {
                script: "deploy.sh".into(),
            },
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
            DescribeArgs {
                script: "bare.sh".into(),
            },
            false,
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn build_payload_normalizes_windows_relative_separators() {
        let root = PathBuf::from(r"C:\workspace\scripts");
        let script = PathBuf::from(r"C:\workspace\scripts\tools\deploy.sh");

        let payload = build_payload(&script, &root, &test_schema());

        assert_eq!(payload.relative_path, "tools/deploy.sh");
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
