use crate::domain::Schema;
use crate::lua_widget::WidgetData;
use crate::ports::WorkspaceEntry;
use ratatui::widgets::ListState;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use super::super::app::SchemaPreview;

#[derive(Debug)]
pub(crate) struct WidgetLoadResult {
    pub(crate) widget: Option<WidgetData>,
    pub(crate) error: Option<String>,
}

pub(crate) struct NavigationState {
    pub(crate) current_dir: PathBuf,
    pub(crate) entries: Vec<WorkspaceEntry>,
    pub(crate) list_state: ListState,
    pub(crate) selection: usize,
    pub(crate) widget: Option<WidgetData>,
    pub(crate) widget_error: Option<String>,
    pub(crate) widget_loading: bool,
    pub(crate) widget_receiver: Option<Receiver<WidgetLoadResult>>,
    pub(crate) schema_preview: Option<SchemaPreview>,
    pub(crate) schema_preview_error: Option<String>,
    pub(crate) schema_preview_scroll: u16,
    pub(crate) preview_script: Option<PathBuf>,
    pub(crate) schema_cache: Option<(PathBuf, Schema)>,
}

impl NavigationState {
    pub(crate) fn new(current_dir: PathBuf, entries: Vec<WorkspaceEntry>) -> Self {
        let mut list_state = ListState::default();
        if !entries.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            current_dir,
            entries,
            list_state,
            selection: 0,
            widget: None,
            widget_error: None,
            widget_loading: false,
            widget_receiver: None,
            schema_preview: None,
            schema_preview_error: None,
            schema_preview_scroll: 0,
            preview_script: None,
            schema_cache: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{WorkspaceEntry, WorkspaceEntryKind};

    #[test]
    fn test_new_empty_entries() {
        let state = NavigationState::new(PathBuf::from("/scripts"), vec![]);
        assert_eq!(state.selection, 0);
        assert!(state.entries.is_empty());
        assert!(state.list_state.selected().is_none());
    }

    #[test]
    fn test_new_with_entries_selects_first() {
        let entries = vec![WorkspaceEntry {
            path: PathBuf::from("/scripts/deploy.sh"),
            kind: WorkspaceEntryKind::Script,
        }];
        let state = NavigationState::new(PathBuf::from("/scripts"), entries);
        assert_eq!(state.selection, 0);
        assert_eq!(state.list_state.selected(), Some(0));
    }

    #[test]
    fn test_new_defaults() {
        let state = NavigationState::new(PathBuf::from("/"), vec![]);
        assert!(state.widget.is_none());
        assert!(state.widget_error.is_none());
        assert!(!state.widget_loading);
        assert!(state.schema_preview.is_none());
        assert!(state.schema_cache.is_none());
    }
}
