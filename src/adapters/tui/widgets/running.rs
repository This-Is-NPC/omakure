use ratatui::layout::{Alignment, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::App;

pub(crate) fn render_running(frame: &mut Frame, area: Rect, app: &mut App) {
    let script_name = app
        .selected_script
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>");
    let args = if app.args.is_empty() {
        "-".to_string()
    } else {
        app.args.join(" ")
    };

    let lines = vec![
        Line::from("Running script..."),
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
