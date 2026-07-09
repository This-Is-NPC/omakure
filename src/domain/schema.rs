use serde::{Deserialize, Serialize};

use crate::error::SchemaError;

/// Schema definition for a script.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Schema {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub fields: Vec<Field>,
    pub outputs: Option<Vec<OutputField>>,
    pub queue: Option<QueueSpec>,
    pub schedule: Option<Schedule>,
}

/// Optional scheduling block that promotes a script to a scheduled automation unit.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Schedule {
    pub cron: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Schedule {
    /// Validate the cron expression. Called by `Schema::validate`.
    pub fn validate(&self) -> Result<(), SchemaError> {
        crate::domain::schedule::parse_cron(&self.cron).map(|_| ())
    }
}

/// Script input field definition.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Field {
    pub name: String,
    pub prompt: Option<String>,
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(default)]
    pub order: Option<u32>,
    pub required: Option<bool>,
    pub default: Option<String>,
    pub choices: Option<Vec<String>>,
    pub arg: Option<String>,
}

impl Field {
    pub fn is_secret(&self) -> bool {
        self.kind.eq_ignore_ascii_case("secret")
    }
}

impl Schema {
    /// Fill any `Field.order` left as `None` with its 1-based declaration
    /// index, leaving explicit orders untouched. Idempotent.
    pub fn normalize_field_orders(&mut self) {
        for (index, field) in self.fields.iter_mut().enumerate() {
            if field.order.is_none() {
                field.order = Some((index as u32) + 1);
            }
        }
    }

    /// Run post-parse validations that serde cannot express (e.g. cron syntax).
    pub fn validate(&self) -> Result<(), SchemaError> {
        if let Some(schedule) = &self.schedule {
            schedule.validate()?;
        }
        for field in &self.fields {
            if field.is_secret() && field.choices.is_some() {
                return Err(SchemaError::UnsupportedSecretFieldConstruct {
                    field: field.name.clone(),
                    construct: "Choices",
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::parse_schema;

    #[test]
    fn normalize_fills_missing_orders_with_declaration_index() {
        let json = r#"{
            "Name": "no_order",
            "Fields": [
                { "Name": "a", "Type": "string" },
                { "Name": "b", "Type": "string" },
                { "Name": "c", "Type": "string" }
            ]
        }"#;
        let mut schema = parse_schema(json).unwrap();
        assert!(schema.fields.iter().all(|f| f.order.is_none()));
        schema.normalize_field_orders();
        assert_eq!(
            schema.fields.iter().map(|f| f.order).collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn normalize_preserves_explicit_orders_and_fills_gaps() {
        let json = r#"{
            "Name": "mixed",
            "Fields": [
                { "Name": "a", "Type": "string", "Order": 10 },
                { "Name": "b", "Type": "string" },
                { "Name": "c", "Type": "string", "Order": 30 }
            ]
        }"#;
        let mut schema = parse_schema(json).unwrap();
        schema.normalize_field_orders();
        assert_eq!(
            schema.fields.iter().map(|f| f.order).collect::<Vec<_>>(),
            vec![Some(10), Some(2), Some(30)]
        );
    }

    #[test]
    fn schedule_block_parses_with_defaults() {
        let json = r#"{
            "Name": "s",
            "Fields": [],
            "Schedule": { "Cron": "@hourly" }
        }"#;
        let schema = parse_schema(json).unwrap();
        let schedule = schema.schedule.unwrap();
        assert_eq!(schedule.cron, "@hourly");
        assert!(schedule.enabled);
    }

    #[test]
    fn schedule_enabled_false_parses() {
        let json = r#"{
            "Name": "s",
            "Fields": [],
            "Schedule": { "Cron": "*/5 * * * *", "Enabled": false }
        }"#;
        let schema = parse_schema(json).unwrap();
        assert!(!schema.schedule.unwrap().enabled);
    }

    #[test]
    fn invalid_cron_blocks_script_load() {
        let json = r#"{
            "Name": "s",
            "Fields": [],
            "Schedule": { "Cron": "99 * * * *" }
        }"#;
        let err = parse_schema(json).unwrap_err();
        assert!(matches!(err, crate::error::SchemaError::InvalidCron { .. }));
    }

    #[test]
    fn schedule_absent_is_fine() {
        let json = r#"{ "Name": "s", "Fields": [] }"#;
        let schema = parse_schema(json).unwrap();
        assert!(schema.schedule.is_none());
    }

    #[test]
    fn secret_field_parses_and_serializes_as_schema_type() {
        let json = r#"{
            "Name": "secrets",
            "Fields": [
                { "Name": "token", "Type": "secret", "Required": true }
            ]
        }"#;
        let schema = parse_schema(json).unwrap();
        assert_eq!(schema.fields[0].kind, "secret");

        let serialized = serde_json::to_value(&schema).unwrap();
        assert_eq!(serialized["Fields"][0]["Type"], "secret");
    }

    #[test]
    fn secret_field_rejects_plaintext_choices() {
        let json = r#"{
            "Name": "secrets",
            "Fields": [
                { "Name": "token", "Type": "secret", "Choices": ["one", "two"] }
            ]
        }"#;
        let err = parse_schema(json).unwrap_err();
        assert!(matches!(
            err,
            crate::error::SchemaError::UnsupportedSecretFieldConstruct { .. }
        ));
    }

    #[test]
    fn normalize_is_idempotent() {
        let json = r#"{
            "Name": "idem",
            "Fields": [
                { "Name": "a", "Type": "string" }
            ]
        }"#;
        let mut schema = parse_schema(json).unwrap();
        schema.normalize_field_orders();
        let first = schema.fields[0].order;
        schema.normalize_field_orders();
        assert_eq!(schema.fields[0].order, first);
    }
}

/// Script output field definition.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct OutputField {
    pub name: String,
    #[serde(rename = "Type")]
    pub kind: String,
}

/// Optional queue specification for batch execution.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct QueueSpec {
    pub matrix: Option<MatrixSpec>,
    pub cases: Option<Vec<QueueCase>>,
}

/// Matrix specification for batch execution.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MatrixSpec {
    pub values: Vec<MatrixValue>,
}

/// Matrix value.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MatrixValue {
    pub name: String,
    pub values: Vec<String>,
}

/// Queue case entry.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct QueueCase {
    pub name: Option<String>,
    pub values: Vec<CaseValue>,
}

/// Queue case value.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct CaseValue {
    pub name: String,
    pub value: String,
}
