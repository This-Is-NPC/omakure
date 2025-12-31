use ratatui::layout::{Alignment, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub(crate) fn render_loading(frame: &mut Frame, area: Rect) {
    let lines = vec![Line::from("Loading environment..."), Line::from("Please wait.")];
    let block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Loading"))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(block, area);
}
