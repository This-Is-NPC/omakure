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
