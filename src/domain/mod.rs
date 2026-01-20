use serde::Deserialize;
use std::error::Error;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Schema {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Field {
    pub name: String,
    pub prompt: Option<String>,
    #[serde(rename = "Type")]
    pub kind: String,
    pub order: u32,
    pub required: Option<bool>,
    pub default: Option<String>,
    pub choices: Option<Vec<String>>,
    pub arg: Option<String>,
}

pub fn parse_schema(output: &str) -> Result<Schema, Box<dyn Error>> {
    for (start, _) in output.match_indices('{') {
        let json = &output[start..];
        let mut deserializer = serde_json::Deserializer::from_str(json);
        if let Ok(schema) = Schema::deserialize(&mut deserializer) {
            return Ok(schema);
        }
    }

    Err("Schema JSON object not found in output".into())
}

pub fn extract_schema_block(contents: &str, prefixes: &[&str]) -> Result<String, String> {
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
                    return Err("Schema block is empty".to_string());
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
            return Err(format!(
                "Schema block line missing comment prefix at line {}",
                index + 1
            ));
        }
    }

    Err("Schema block not found".to_string())
}

fn strip_comment_prefix<'a>(line: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    let trimmed = line.trim_start();
    for prefix in prefixes {
        if trimmed.starts_with(prefix) {
            let mut remainder = &trimmed[prefix.len()..];
            if remainder.starts_with(' ') {
                remainder = &remainder[1..];
            }
            return Some(remainder);
        }
    }
    None
}

pub fn normalize_input(field: &Field, input: &str) -> Result<Option<String>, String> {
    let trimmed = input.trim();
    let required = field.required.unwrap_or(false);
    let default_value = field.default.as_deref();

    let raw_value = if trimmed.is_empty() {
        if let Some(default_value) = default_value {
            default_value.to_string()
        } else if required {
            return Err("Value required".to_string());
        } else {
            return Ok(None);
        }
    } else {
        trimmed.to_string()
    };

    if let Some(choices) = &field.choices {
        if !choices.iter().any(|choice| choice == &raw_value) {
            return Err(format!("Allowed values: {}", choices.join(", ")));
        }
    }

    let kind = field.kind.to_lowercase();
    match kind.as_str() {
        "string" => Ok(Some(raw_value)),
        "number" => {
            if raw_value.parse::<f64>().is_err() {
                return Err("Enter a valid number".to_string());
            }
            Ok(Some(raw_value))
        }
        "bool" | "boolean" => match parse_bool(&raw_value) {
            Some(value) => Ok(Some(value.to_string())),
            None => Err("Enter true/false (or yes/no)".to_string()),
        },
        _ => Ok(Some(raw_value)),
    }
}

fn parse_bool(input: &str) -> Option<bool> {
    match input.trim().to_lowercase().as_str() {
        "true" | "t" | "yes" | "y" | "1" => Some(true),
        "false" | "f" | "no" | "n" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_field(name: &str, kind: &str, required: bool) -> Field {
        Field {
            name: name.to_string(),
            prompt: None,
            kind: kind.to_string(),
            order: 1,
            required: Some(required),
            default: None,
            choices: None,
            arg: None,
        }
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
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize_input_string() {
        let field = make_field("name", "string", false);
        let result = normalize_input(&field, "  hello world  ").unwrap();
        assert_eq!(result, Some("hello world".to_string()));
    }

    #[test]
    fn test_normalize_input_empty_optional() {
        let field = make_field("name", "string", false);
        let result = normalize_input(&field, "").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_normalize_input_empty_required() {
        let field = make_field("name", "string", true);
        let result = normalize_input(&field, "");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Value required");
    }

    #[test]
    fn test_normalize_input_with_default() {
        let mut field = make_field("name", "string", false);
        field.default = Some("default_value".to_string());
        let result = normalize_input(&field, "").unwrap();
        assert_eq!(result, Some("default_value".to_string()));
    }

    #[test]
    fn test_normalize_input_number_valid() {
        let field = make_field("count", "number", false);
        let result = normalize_input(&field, "42").unwrap();
        assert_eq!(result, Some("42".to_string()));

        let result = normalize_input(&field, "3.14").unwrap();
        assert_eq!(result, Some("3.14".to_string()));
    }

    #[test]
    fn test_normalize_input_number_invalid() {
        let field = make_field("count", "number", false);
        let result = normalize_input(&field, "not a number");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Enter a valid number");
    }

    #[test]
    fn test_normalize_input_bool_valid() {
        let field = make_field("flag", "bool", false);

        assert_eq!(
            normalize_input(&field, "true").unwrap(),
            Some("true".to_string())
        );
        assert_eq!(
            normalize_input(&field, "yes").unwrap(),
            Some("true".to_string())
        );
        assert_eq!(
            normalize_input(&field, "Y").unwrap(),
            Some("true".to_string())
        );
        assert_eq!(
            normalize_input(&field, "1").unwrap(),
            Some("true".to_string())
        );

        assert_eq!(
            normalize_input(&field, "false").unwrap(),
            Some("false".to_string())
        );
        assert_eq!(
            normalize_input(&field, "no").unwrap(),
            Some("false".to_string())
        );
        assert_eq!(
            normalize_input(&field, "N").unwrap(),
            Some("false".to_string())
        );
        assert_eq!(
            normalize_input(&field, "0").unwrap(),
            Some("false".to_string())
        );
    }

    #[test]
    fn test_normalize_input_bool_invalid() {
        let field = make_field("flag", "bool", false);
        let result = normalize_input(&field, "maybe");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Enter true/false (or yes/no)");
    }

    #[test]
    fn test_normalize_input_with_choices() {
        let mut field = make_field("env", "string", false);
        field.choices = Some(vec!["dev".to_string(), "prod".to_string()]);

        let result = normalize_input(&field, "dev").unwrap();
        assert_eq!(result, Some("dev".to_string()));

        let result = normalize_input(&field, "staging");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Allowed values"));
    }
}
