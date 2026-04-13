use crate::error::SchemaError;

use super::schema::Field;

/// Normalizes and validates a field input value.
pub fn normalize_input(field: &Field, input: &str) -> Result<Option<String>, SchemaError> {
    let trimmed = input.trim();
    let required = field.required.unwrap_or(false);
    let default_value = field.default.as_deref();

    let raw_value = if trimmed.is_empty() {
        if let Some(default_value) = default_value {
            default_value.to_string()
        } else if required {
            return Err(SchemaError::ValueRequired);
        } else {
            return Ok(None);
        }
    } else {
        trimmed.to_string()
    };

    if let Some(choices) = &field.choices {
        if !choices.iter().any(|choice| choice == &raw_value) {
            return Err(SchemaError::InvalidChoice {
                choices: choices.join(", "),
            });
        }
    }

    let kind = field.kind.to_lowercase();
    match kind.as_str() {
        "string" => Ok(Some(raw_value)),
        "number" => {
            if raw_value.parse::<f64>().is_err() {
                return Err(SchemaError::InvalidNumber);
            }
            Ok(Some(raw_value))
        }
        "bool" | "boolean" => match parse_bool(&raw_value) {
            Some(value) => Ok(Some(value.to_string())),
            None => Err(SchemaError::InvalidBoolean),
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
        assert!(matches!(result.unwrap_err(), SchemaError::ValueRequired));
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
        assert!(matches!(result.unwrap_err(), SchemaError::InvalidNumber));
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
        assert!(matches!(result.unwrap_err(), SchemaError::InvalidBoolean));
    }

    #[test]
    fn test_normalize_input_unknown_kind_passes_through() {
        let field = make_field("anything", "weird", false);
        let result = normalize_input(&field, "value").unwrap();
        assert_eq!(result, Some("value".to_string()));
    }

    #[test]
    fn test_normalize_input_with_choices() {
        let mut field = make_field("env", "string", false);
        field.choices = Some(vec!["dev".to_string(), "prod".to_string()]);

        let result = normalize_input(&field, "dev").unwrap();
        assert_eq!(result, Some("dev".to_string()));

        let result = normalize_input(&field, "staging");
        assert!(matches!(
            result.unwrap_err(),
            SchemaError::InvalidChoice { .. }
        ));
    }
}
