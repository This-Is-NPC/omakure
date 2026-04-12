use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::App;
use super::super::theme::Theme;
use super::spinner::{spinner_span, SpinnerKind};

pub(crate) fn render_running(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let script_name = app
        .field_input
        .selected_script
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>");
    let args = if app.field_input.args.is_empty() {
        "-".to_string()
    } else {
        app.field_input.args.join(" ")
    };

    let lines = vec![
        Line::from(vec![
            spinner_span(SpinnerKind::Sand, app.tick, theme),
            Span::raw("Running script..."),
        ]),
        Line::from(""),
        Line::from(format!("Script: {}", script_name)),
        Line::from(format!("Args: {}", args)),
        Line::from(""),
        Line::from("Please wait."),
    ];
    let block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Executing"))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(block, area);
}
