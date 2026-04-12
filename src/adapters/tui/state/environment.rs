use crate::adapters::environments::{EnvFile, EnvironmentConfig};
use ratatui::widgets::ListState;

pub(crate) struct EnvironmentState {
    pub(crate) config: Option<EnvironmentConfig>,
    pub(crate) error: Option<String>,
    pub(crate) entries: Vec<EnvFile>,
    pub(crate) list_state: ListState,
    pub(crate) selection: usize,
    pub(crate) preview_lines: Vec<ratatui::text::Line<'static>>,
    pub(crate) preview_error: Option<String>,
    pub(crate) preview_scroll: u16,
}

impl EnvironmentState {
    pub(crate) fn new() -> Self {
        Self {
            config: None,
            error: None,
            entries: Vec::new(),
            list_state: ListState::default(),
            selection: 0,
            preview_lines: Vec::new(),
            preview_error: None,
            preview_scroll: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let state = EnvironmentState::new();
        assert!(state.config.is_none());
        assert!(state.error.is_none());
        assert!(state.entries.is_empty());
        assert_eq!(state.selection, 0);
        assert!(state.preview_lines.is_empty());
        assert!(state.preview_error.is_none());
        assert_eq!(state.preview_scroll, 0);
    }
}
