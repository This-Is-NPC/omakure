use crate::app_meta;
use crate::lua_widget::WidgetData;
use crate::workspace::Workspace;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::theme::Theme;
use super::spinner::{spinner_span, SpinnerKind};

pub(crate) fn render_environment(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
) {
    let info_block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: true });
    frame.render_widget(info_block, area);
}

pub(crate) fn status_info(
    workspace: &Workspace,
    widget: Option<&WidgetData>,
    widget_error: Option<&str>,
    widget_loading: bool,
    theme: &Theme,
    tick: u64,
) -> (String, Vec<Line<'static>>) {
    if widget_loading {
        return (
            "Loading".to_string(),
            vec![
                Line::from(vec![
                    spinner_span(SpinnerKind::Sand, tick, theme),
                    Span::raw("Loading environment..."),
                ]),
                Line::from("Please wait."),
            ],
        );
    }

    if let Some(widget) = widget {
        let lines = widget
            .lines
            .iter()
            .map(|line| Line::from(line.clone()))
            .collect();
        return (widget.title.clone(), lines);
    }

    if let Some(message) = widget_error {
        return (
            "Widget Error".to_string(),
            vec![
                Line::from("Failed to load index.lua."),
                Line::from(message.to_string()),
            ],
        );
    }

    let mut lines = Vec::new();
    lines.push(Line::from(format!("Root: {}", workspace.root().display())));
    lines.push(Line::from(format!("Version: v{}", app_meta::APP_VERSION)));
    let repo = if app_meta::REPO_URL.is_empty() {
        "<unknown>"
    } else {
        app_meta::REPO_URL
    };
    lines.push(Line::from(format!("Repo: {}", repo)));
    ("Workspace".to_string(), lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tempfile::TempDir;

    #[test]
    fn snapshot_render_environment() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let lines = vec![
            Line::from("Root: /home/user/scripts"),
            Line::from("Version: v0.1.8"),
        ];
        terminal
            .draw(|f| {
                render_environment(f, f.size(), "Workspace", lines);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn test_status_info_default_workspace() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let theme = Theme::default();
        let (title, lines) = status_info(&ws, None, None, false, &theme, 0);
        assert_eq!(title, "Workspace");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_status_info_loading() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let theme = Theme::default();
        let (title, _) = status_info(&ws, None, None, true, &theme, 0);
        assert_eq!(title, "Loading");
    }

    #[test]
    fn test_status_info_widget_error() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let theme = Theme::default();
        let (title, lines) = status_info(&ws, None, Some("lua error"), false, &theme, 0);
        assert_eq!(title, "Widget Error");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_status_info_with_widget() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let theme = Theme::default();
        let widget = WidgetData {
            title: "Custom".to_string(),
            lines: vec!["Line 1".to_string(), "Line 2".to_string()],
        };
        let (title, lines) = status_info(&ws, Some(&widget), None, false, &theme, 0);
        assert_eq!(title, "Custom");
        assert_eq!(lines.len(), 2);
    }
}
