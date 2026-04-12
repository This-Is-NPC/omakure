use ratatui::layout::Rect;

use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::path::Path;

use super::super::theme::Theme;
use crate::ports::{WorkspaceEntry, WorkspaceEntryKind};
use crate::workspace::Workspace;

pub(crate) fn render_scripts(
    frame: &mut Frame,
    area: Rect,
    workspace: &Workspace,
    current_dir: &Path,
    entries: &[WorkspaceEntry],
    list_state: &mut ListState,
    theme: &Theme,
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
            .highlight_style(theme.selection_style())
            .highlight_symbol(super::super::theme::selection_symbol_str());

        frame.render_stateful_widget(list, area, list_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn snapshot_scripts_list() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let entries = vec![
            WorkspaceEntry {
                path: PathBuf::from("/scripts/infra"),
                kind: WorkspaceEntryKind::Directory,
            },
            WorkspaceEntry {
                path: PathBuf::from("/scripts/deploy.sh"),
                kind: WorkspaceEntryKind::Script,
            },
            WorkspaceEntry {
                path: PathBuf::from("/scripts/setup.py"),
                kind: WorkspaceEntryKind::Script,
            },
        ];
        let mut list_state = ListState::default();
        list_state.select(Some(1));
        let theme = Theme::default();

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_scripts(
                    f,
                    f.size(),
                    &ws,
                    tmp.path(),
                    &entries,
                    &mut list_state,
                    &theme,
                );
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_scripts_empty() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut list_state = ListState::default();
        let theme = Theme::default();

        let backend = TestBackend::new(50, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_scripts(
                    f,
                    f.size(),
                    &ws,
                    tmp.path(),
                    &[],
                    &mut list_state,
                    &theme,
                );
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
