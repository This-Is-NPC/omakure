use ratatui::layout::Rect;

use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::path::Path;

use super::super::theme;
use crate::ports::{WorkspaceEntry, WorkspaceEntryKind};
use crate::workspace::Workspace;

pub(crate) fn render_scripts(
    frame: &mut Frame,
    area: Rect,
    workspace: &Workspace,
    current_dir: &Path,
    entries: &[WorkspaceEntry],
    list_state: &mut ListState,
) {
    if entries.is_empty() {
        let relative = current_dir
            .strip_prefix(workspace.root())
            .unwrap_or(current_dir)
            .to_string_lossy();
        let current_label = if relative.is_empty() { "." } else { &relative };
        let empty_lines = vec![
            Line::from("No scripts or folders found."),
            Line::from(format!("Directory: {}", current_label)),
            Line::from("Add scripts or folders and press r to refresh."),
        ];
        let empty = Paragraph::new(empty_lines)
            .block(Block::default().borders(Borders::ALL).title("Entries"))
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
    } else {
        let items: Vec<ListItem> = entries
            .iter()
            .map(|entry| {
                let name = entry
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("<unknown>");
                let label = match entry.kind {
                    WorkspaceEntryKind::Directory => format!("{}/", name),
                    WorkspaceEntryKind::Script => name.to_string(),
                };
                ListItem::new(label)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Entries"))
            .highlight_style(theme::selection_style())
            .highlight_symbol(theme::selection_symbol_str());

        frame.render_stateful_widget(list, area, list_state);
    }
}
