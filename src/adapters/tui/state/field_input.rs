use crate::domain::Field;
use std::path::PathBuf;

pub(crate) struct FieldInputState {
    pub(crate) schema_name: Option<String>,
    pub(crate) schema_description: Option<String>,
    pub(crate) fields: Vec<Field>,
    pub(crate) field_index: usize,
    pub(crate) field_inputs: Vec<String>,
    pub(crate) args: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) selected_script: Option<PathBuf>,
}

impl FieldInputState {
    pub(crate) fn new() -> Self {
        Self {
            schema_name: None,
            schema_description: None,
            fields: Vec::new(),
            field_index: 0,
            field_inputs: Vec::new(),
            args: Vec::new(),
            error: None,
            selected_script: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let state = FieldInputState::new();
        assert!(state.schema_name.is_none());
        assert!(state.fields.is_empty());
        assert_eq!(state.field_index, 0);
        assert!(state.field_inputs.is_empty());
        assert!(state.args.is_empty());
        assert!(state.error.is_none());
        assert!(state.selected_script.is_none());
    }
}
