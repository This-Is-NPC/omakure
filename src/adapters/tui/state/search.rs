use crate::search_index::{SearchDetails, SearchResult, SearchStatus};
use ratatui::widgets::ListState;

pub(crate) struct SearchState {
    pub(crate) query: String,
    pub(crate) results: Vec<SearchResult>,
    pub(crate) list_state: ListState,
    pub(crate) selection: usize,
    pub(crate) details: Option<SearchDetails>,
    pub(crate) status: SearchStatus,
    pub(crate) error: Option<String>,
}

impl SearchState {
    pub(crate) fn new(status: SearchStatus) -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            selection: 0,
            details: None,
            status,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let state = SearchState::new(SearchStatus::Idle);
        assert!(state.query.is_empty());
        assert!(state.results.is_empty());
        assert_eq!(state.selection, 0);
        assert!(state.details.is_none());
        assert_eq!(state.status, SearchStatus::Idle);
        assert!(state.error.is_none());
    }
}
