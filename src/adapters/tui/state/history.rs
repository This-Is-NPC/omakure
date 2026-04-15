use crate::runs::RunRow;
use ratatui::widgets::TableState;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum HistoryFocus {
    List,
    Output,
}

/// Which subview of the History screen is currently rendered. The
/// classic table-of-runs lives in [`HistoryView::List`]; the new charts
/// view lives in [`HistoryView::Dashboards`]. `Tab` cycles between
/// them. The two views share the same `selection` so the per-script
/// dashboard panel always tracks the row highlighted in `List`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum HistoryView {
    List,
    Dashboards,
}

/// Layout mode of the dashboards view. `Split` shows global panels
/// stacked on top of the per-script panel; `ExpandedPerScript`
/// promotes the per-script panel to fill the entire dashboards body.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum DashboardLayout {
    Split,
    ExpandedPerScript,
}

pub(crate) struct HistoryState {
    pub(crate) entries: Vec<RunRow>,
    pub(crate) table_state: TableState,
    pub(crate) selection: usize,
    pub(crate) focus: HistoryFocus,
    pub(crate) view: HistoryView,
    pub(crate) dashboard_layout: DashboardLayout,
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
            view: HistoryView::List,
            dashboard_layout: DashboardLayout::Split,
        }
    }

    /// Replace `entries` in place while preserving view, focus, layout,
    /// and — when possible — the user's current selection. Used by the
    /// TUI's background auto-refresh so a refresh tick doesn't kick the
    /// user out of Dashboards view or scroll them back to row 0.
    pub(crate) fn replace_entries(&mut self, entries: Vec<RunRow>) {
        let new_len = entries.len();
        self.entries = entries;
        if new_len == 0 {
            self.selection = 0;
            self.table_state.select(None);
        } else {
            if self.selection >= new_len {
                self.selection = new_len - 1;
            }
            self.table_state.select(Some(self.selection));
        }
    }

    /// `Tab`: cycle between the table view and the dashboards view.
    /// Switching views never resets the selection so the per-script
    /// dashboard panel always tracks the highlighted row.
    pub(crate) fn toggle_view(&mut self) {
        self.view = match self.view {
            HistoryView::List => HistoryView::Dashboards,
            HistoryView::Dashboards => HistoryView::List,
        };
    }

    /// `e`/`Enter` inside Dashboards: promote the per-script panel to
    /// fill the dashboards body. Pressing the same key again collapses
    /// it back to the split layout.
    pub(crate) fn toggle_dashboard_expand(&mut self) {
        self.dashboard_layout = match self.dashboard_layout {
            DashboardLayout::Split => DashboardLayout::ExpandedPerScript,
            DashboardLayout::ExpandedPerScript => DashboardLayout::Split,
        };
    }

    /// `Esc` inside Dashboards. Collapses an expanded panel first and
    /// reports `false`; only when already in the split layout does it
    /// report `true` to signal the caller should leave the screen.
    pub(crate) fn dashboards_escape(&mut self) -> bool {
        if self.dashboard_layout == DashboardLayout::ExpandedPerScript {
            self.dashboard_layout = DashboardLayout::Split;
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> HistoryState {
        // A non-empty list so the dashboard panel has a selected row.
        let mut state = HistoryState::new(Vec::new());
        // Bypass the empty-input branch — the toggle helpers don't read
        // entries, only the layout fields.
        state.selection = 0;
        state
    }

    #[test]
    fn toggle_view_cycles_list_and_dashboards() {
        let mut state = fixture();
        assert_eq!(state.view, HistoryView::List);
        state.toggle_view();
        assert_eq!(state.view, HistoryView::Dashboards);
        state.toggle_view();
        assert_eq!(state.view, HistoryView::List);
    }

    #[test]
    fn toggle_dashboard_expand_cycles_layout() {
        let mut state = fixture();
        assert_eq!(state.dashboard_layout, DashboardLayout::Split);
        state.toggle_dashboard_expand();
        assert_eq!(state.dashboard_layout, DashboardLayout::ExpandedPerScript);
        state.toggle_dashboard_expand();
        assert_eq!(state.dashboard_layout, DashboardLayout::Split);
    }

    #[test]
    fn dashboards_escape_collapses_first_then_exits() {
        let mut state = fixture();
        state.toggle_dashboard_expand();
        assert_eq!(state.dashboard_layout, DashboardLayout::ExpandedPerScript);
        // First Esc collapses but does not exit.
        assert!(!state.dashboards_escape());
        assert_eq!(state.dashboard_layout, DashboardLayout::Split);
        // Second Esc reports exit.
        assert!(state.dashboards_escape());
    }

    #[test]
    fn view_state_persists_across_struct_use() {
        // The state machine has no automatic resets — entering and
        // leaving the History screen reuses the same HistoryState
        // instance, so toggles persist between visits unless the caller
        // explicitly resets them.
        let mut state = fixture();
        state.toggle_view();
        state.toggle_dashboard_expand();
        // Simulate a screen-switch round-trip by simply not touching
        // `state` between mutations.
        assert_eq!(state.view, HistoryView::Dashboards);
        assert_eq!(state.dashboard_layout, DashboardLayout::ExpandedPerScript);
    }
}
