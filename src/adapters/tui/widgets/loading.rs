use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::theme::Theme;
use super::spinner::{spinner_span, SpinnerKind};

pub(crate) fn render_loading(frame: &mut Frame, area: Rect, theme: &Theme, tick: u64) {
    let lines = vec![
        Line::from(vec![
            spinner_span(SpinnerKind::Sand, tick, theme),
            Span::raw("Loading environment..."),
        ]),
        Line::from("Please wait."),
    ];
    let block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Loading"))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(block, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn snapshot_loading_screen() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_loading(f, f.size(), &theme, 0);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
