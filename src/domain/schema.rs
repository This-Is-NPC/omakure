use serde::Deserialize;

/// Schema definition for a script.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct Schema {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub fields: Vec<Field>,
    pub outputs: Option<Vec<OutputField>>,
    pub queue: Option<QueueSpec>,
}

/// Script input field definition.
#[derive(Debug, Deserialize, Clone)]
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
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct OutputField {
    pub name: String,
    #[serde(rename = "Type")]
    pub kind: String,
}

/// Optional queue specification for batch execution.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct QueueSpec {
    pub matrix: Option<MatrixSpec>,
    pub cases: Option<Vec<QueueCase>>,
}

/// Matrix specification for batch execution.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MatrixSpec {
    pub values: Vec<MatrixValue>,
}

/// Matrix value.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MatrixValue {
    pub name: String,
    pub values: Vec<String>,
}

/// Queue case entry.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct QueueCase {
    pub name: Option<String>,
    pub values: Vec<CaseValue>,
}

/// Queue case value.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct CaseValue {
    pub name: String,
    pub value: String,
}
