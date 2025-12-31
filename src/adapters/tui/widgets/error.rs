use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub(crate) fn render_error(frame: &mut Frame, area: Rect, message: &str) {
    let lines = vec![
        Line::from(Span::styled(message, Style::default().fg(Color::Red))),
        Line::from(""),
        Line::from("Press Enter to return, Esc to quit"),
    ];
    let block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Error"))
        .wrap(Wrap { trim: true });
    frame.render_widget(block, area);
}
