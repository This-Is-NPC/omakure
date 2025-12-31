use serde::Deserialize;
use std::error::Error;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Schema {
    pub name: String,
    pub description: Option<String>,
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
