use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use crate::lua_widget::WidgetData;
use crate::workspace::Workspace;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_URL: &str = "https://github.com/This-Is-NPC/omakure";

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
) -> (String, Vec<Line<'static>>) {
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
    lines.push(Line::from(format!(
        "Root: {}",
        workspace.root().display()
    )));
    lines.push(Line::from(format!("Version: v{}", APP_VERSION)));
    lines.push(Line::from(format!("Repo: {}", REPO_URL)));
    ("Workspace".to_string(), lines)
}
