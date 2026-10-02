use serde::{Deserialize, de::Error as _};
use serde_json::Value;

use crate::error::SchemaError;

use super::schema::Schema;

/// Parses a schema JSON object from a string.
pub fn parse_schema(output: &str) -> Result<Schema, SchemaError> {
    for (start, _) in output.match_indices('{') {
        if let Some(schema) = parse_schema_candidate(&output[start..])? {
            return Ok(schema);
        }
    }

    Err(SchemaError::JsonNotFound)
}

fn parse_schema_candidate(json: &str) -> Result<Option<Schema>, SchemaError> {
    let mut probe = serde_json::Deserializer::from_str(json);
    let Ok(Value::Object(object)) = Value::deserialize(&mut probe) else {
        return Ok(None);
    };
    if !object.contains_key("Name") || !object.contains_key("Fields") {
        return Ok(None);
    }
    for section in ["Outputs", "Queue"] {
        if object.contains_key(section) {
            return Err(SchemaError::InvalidJson(serde_json::Error::custom(
                format!("unsupported schema section: {section}"),
            )));
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let schema = Schema::deserialize(&mut deserializer)?;
    schema.validate()?;
    Ok(Some(schema))
}

/// Extracts the schema block from a script file.
pub fn extract_schema_block(contents: &str, prefixes: &[&str]) -> Result<String, SchemaError> {
    let mut in_block = false;
    let mut buffer = String::new();

    for (index, line) in contents.lines().enumerate() {
        if let Some(commented) = strip_comment_prefix(line, prefixes) {
            if let Some(block) = consume_schema_comment(commented, &mut in_block, &mut buffer)? {
                return Ok(block);
            }
        } else if in_block {
            if line.trim().is_empty() {
                continue;
            }
            return Err(SchemaError::MissingCommentPrefix { line: index + 1 });
        }
    }

    Err(SchemaError::BlockNotFound)
}

fn consume_schema_comment(
    commented: &str,
    in_block: &mut bool,
    buffer: &mut String,
) -> Result<Option<String>, SchemaError> {
    let trimmed = commented.trim();
    if !*in_block {
        if trimmed == "OMAKURE_SCHEMA_START" {
            *in_block = true;
        }
        return Ok(None);
    }
    if trimmed == "OMAKURE_SCHEMA_END" {
        return if buffer.trim().is_empty() {
            Err(SchemaError::EmptyBlock)
        } else {
            Ok(Some(std::mem::take(buffer)))
        };
    }
    if !buffer.is_empty() {
        buffer.push('\n');
    }
    buffer.push_str(commented);
    Ok(None)
}

fn strip_comment_prefix<'a>(line: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    let trimmed = line.trim_start();
    for prefix in prefixes {
        if let Some(stripped) = trimmed.strip_prefix(prefix) {
            let mut remainder = stripped;
            if remainder.starts_with(' ') {
                remainder = &remainder[1..];
            }
            return Some(remainder);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_schema_json() -> String {
        r#"{
  "Name": "test_script",
  "Description": "A test script",
  "Fields": []
}"#
        .to_string()
    }

    fn comment_block(prefix: &str, json: &str) -> String {
        json.lines()
            .map(|line| format!("{} {}", prefix, line))
            .collect::<Vec<String>>()
            .join("\n")
    }

    #[test]
    fn test_parse_schema_valid() {
        let output = r#"Some output before
{
  "Name": "test_script",
  "Description": "A test script",
  "Fields": []
}
Some output after"#;
        let schema = parse_schema(output).unwrap();
        assert_eq!(schema.name, "test_script");
        assert_eq!(schema.description, Some("A test script".to_string()));
        assert!(schema.fields.is_empty());
    }

    #[test]
    fn parse_schema_skips_unrelated_json_before_the_schema() {
        let output = r#"{"event":"ready"}
{"Name":"job","Fields":[]}"#;
        assert_eq!(parse_schema(output).unwrap().name, "job");
    }

    #[test]
    fn test_parse_schema_with_fields() {
        let output = r#"{
  "Name": "my_script",
  "Fields": [
    {
      "Name": "target",
      "Type": "string",
      "Order": 1,
      "Required": true
    }
  ]
}"#;
        let schema = parse_schema(output).unwrap();
        assert_eq!(schema.name, "my_script");
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "target");
        assert_eq!(schema.fields[0].required, Some(true));
    }

    #[test]
    fn test_parse_schema_not_found() {
        let output = "No JSON here";
        let result = parse_schema(output);
        assert!(matches!(result.unwrap_err(), SchemaError::JsonNotFound));
    }

    #[test]
    fn test_extract_schema_block_hash_prefix() {
        let json = make_schema_json();
        let contents = format!(
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n{}\n# OMAKURE_SCHEMA_END",
            comment_block("#", &json)
        );
        let block = extract_schema_block(&contents, &["#"]).unwrap();
        let schema = parse_schema(&block).unwrap();
        assert_eq!(schema.name, "test_script");
    }

    #[test]
    fn test_extract_schema_block_semicolon_prefix() {
        let json = make_schema_json();
        let contents = format!(
            "; OMAKURE_SCHEMA_START\n{}\n; OMAKURE_SCHEMA_END",
            comment_block(";", &json)
        );
        let block = extract_schema_block(&contents, &[";"]).unwrap();
        let schema = parse_schema(&block).unwrap();
        assert_eq!(schema.name, "test_script");
    }

    #[test]
    fn test_extract_schema_block_missing_prefix_line() {
        let contents = "# OMAKURE_SCHEMA_START\n{\n# OMAKURE_SCHEMA_END";
        let result = extract_schema_block(contents, &["#"]);
        assert!(matches!(
            result.unwrap_err(),
            SchemaError::MissingCommentPrefix { .. }
        ));
    }

    #[test]
    fn test_extract_schema_block_empty() {
        let contents = "# OMAKURE_SCHEMA_START\n# OMAKURE_SCHEMA_END";
        let result = extract_schema_block(contents, &["#"]);
        assert!(matches!(result.unwrap_err(), SchemaError::EmptyBlock));
    }

    #[test]
    fn test_extract_schema_block_not_found() {
        let contents = "# Just some code\necho hello";
        let result = extract_schema_block(contents, &["#"]);
        assert!(matches!(result.unwrap_err(), SchemaError::BlockNotFound));
    }

    #[test]
    fn extract_schema_block_ignores_end_before_start_and_returns_first_block() {
        let contents = "# OMAKURE_SCHEMA_END\n# OMAKURE_SCHEMA_START\n# first\n# OMAKURE_SCHEMA_END\n# OMAKURE_SCHEMA_START\n# second\n# OMAKURE_SCHEMA_END";
        assert_eq!(extract_schema_block(contents, &["#"]).unwrap(), "first");
    }

    #[test]
    fn extract_schema_block_skips_unprefixed_blank_lines_but_reports_code_line() {
        let valid = "# OMAKURE_SCHEMA_START\n\n# value\n# OMAKURE_SCHEMA_END";
        assert_eq!(extract_schema_block(valid, &["#"]).unwrap(), "value");

        let invalid = "# OMAKURE_SCHEMA_START\n\nvalue\n# OMAKURE_SCHEMA_END";
        assert!(matches!(
            extract_schema_block(invalid, &["#"]),
            Err(SchemaError::MissingCommentPrefix { line: 3 })
        ));
    }
}
