use crate::runs::RunRow;
use ratatui::widgets::TableState;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum HistoryFocus {
    List,
    Output,
}

pub(crate) struct HistoryState {
    pub(crate) entries: Vec<RunRow>,
    pub(crate) table_state: TableState,
    pub(crate) selection: usize,
    pub(crate) focus: HistoryFocus,
}

impl HistoryState {
    pub(crate) fn new(entries: Vec<RunRow>) -> Self {
        let mut table_state = TableState::default();
        if !entries.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            entries,
            table_state,
            selection: 0,
            focus: HistoryFocus::List,
        }
    }
}
