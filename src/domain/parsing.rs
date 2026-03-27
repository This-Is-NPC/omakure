use serde::Deserialize;

use crate::error::SchemaError;

use super::schema::Schema;

/// Parses a schema JSON object from a string.
pub fn parse_schema(output: &str) -> Result<Schema, SchemaError> {
    for (start, _) in output.match_indices('{') {
        let json = &output[start..];
        let mut deserializer = serde_json::Deserializer::from_str(json);
        if let Ok(schema) = Schema::deserialize(&mut deserializer) {
            return Ok(schema);
        }
    }

    Err(SchemaError::JsonNotFound)
}

/// Extracts the schema block from a script file.
pub fn extract_schema_block(contents: &str, prefixes: &[&str]) -> Result<String, SchemaError> {
    let mut in_block = false;
    let mut buffer = String::new();

    for (index, line) in contents.lines().enumerate() {
        if let Some(commented) = strip_comment_prefix(line, prefixes) {
            let trimmed = commented.trim();
            if !in_block && trimmed == "OMAKURE_SCHEMA_START" {
                in_block = true;
                continue;
            }
            if in_block && trimmed == "OMAKURE_SCHEMA_END" {
                if buffer.trim().is_empty() {
                    return Err(SchemaError::EmptyBlock);
                }
                return Ok(buffer);
            }
            if in_block {
                if !buffer.is_empty() {
                    buffer.push('\n');
                }
                buffer.push_str(commented);
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
}
