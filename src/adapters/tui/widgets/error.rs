use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::theme::Theme;

pub(crate) fn render_error(frame: &mut Frame, area: Rect, message: &str, theme: &Theme) {
    let lines = vec![
        Line::from(Span::styled(
            message,
            Style::default().fg(theme.semantic.error.color()),
        )),
        Line::from(""),
        Line::from("Press Enter to return, Esc to quit"),
    ];
    let block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Error"))
        .wrap(Wrap { trim: true });
    frame.render_widget(block, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn snapshot_error_message() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_error(f, f.size(), "Something went wrong!", &theme);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_error_long_message() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        let msg =
            "A very long error message that should wrap around the available width in the terminal";
        terminal
            .draw(|f| {
                render_error(f, f.size(), msg, &theme);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
