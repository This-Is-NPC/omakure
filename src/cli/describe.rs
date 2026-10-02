//! `omakure describe <script>` — print the full schema of one script.

use crate::cli::args::DescribeArgs;
use crate::cli::emit::emit_operation_error;
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
        Err(err) => return emit_operation_error(json_output, err, describe_error_code),
    };

    if json_output {
        json::print_ok(payload_from_description(description));
        return Ok(());
    }

    print_human_payload(&payload_from_description(description));
    Ok(())
}

fn describe_error_code(err: &OperationError) -> &'static str {
    match err.code {
        OperationErrorCode::NotFound => codes::NOT_FOUND,
        OperationErrorCode::InvalidInput if is_missing_schema_message(&err.message) => {
            codes::NOT_FOUND
        }
        OperationErrorCode::UnsafePath => codes::INVALID_ARGUMENT,
        OperationErrorCode::InvalidInput => codes::SCHEMA_INVALID,
        _ => codes::INTERNAL,
    }
}

fn is_missing_schema_message(message: &str) -> bool {
    message.contains("Schema block not found") || message.contains("Schema JSON object not found")
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
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn describe_payload(schema_json: &str) -> DescribePayload {
        let tmp = TempDir::new().unwrap();
        write_schema_script(
            &tmp,
            "deploy.sh",
            &format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {schema_json}\n# OMAKURE_SCHEMA_END\n"
            ),
        );
        let description = core::describe_script(
            &Workspace::new(tmp.path().to_path_buf()),
            DescribeScriptRequest {
                script: "deploy.sh".into(),
            },
        )
        .unwrap();
        payload_from_description(description)
    }

    #[test]
    fn payload_carries_the_schema_structure() {
        let payload = describe_payload(
            r#"{"Name":"Deploy","Description":"Deploy the app","Tags":["ops"],"Fields":[{"Name":"target","Prompt":"Target env","Type":"string","Order":1,"Required":true,"Choices":["dev","prod"],"Arg":"--target"}]}"#,
        );
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
    fn payload_marks_secret_fields_without_returning_values() {
        let payload = describe_payload(
            r#"{"Name":"Deploy","Fields":[{"Name":"token","Type":"secret","Default":"supersecret"}]}"#,
        );
        assert_eq!(payload.fields[0].kind, "secret");
        assert_eq!(payload.fields[0].default, None);
    }

    #[test]
    fn payload_without_fields_is_empty() {
        let payload = describe_payload(r#"{"Name":"Simple","Fields":[]}"#);
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
    fn test_sample_envelope_shape() {
        let envelope = sample_envelope();
        assert_eq!(envelope["ok"], true);
        assert!(envelope["data"]["name"].is_string());
        assert!(envelope["data"]["fields"].is_array());
        assert_eq!(envelope["schema_version"], "1");
    }
}
