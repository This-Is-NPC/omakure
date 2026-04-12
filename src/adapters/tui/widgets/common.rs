use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};

use super::super::app::ExecutionStatus;
use super::super::theme::Theme;
use crate::runs::RunState;

pub(crate) fn status_label_and_style(status: &ExecutionStatus, theme: &Theme) -> (String, Style) {
    match status {
        ExecutionStatus::Success => ("OK".to_string(), theme.status_ok_style()),
        ExecutionStatus::Failed(code) => match code {
            Some(code) => (format!("FAIL ({})", code), theme.status_fail_style()),
            None => ("FAIL".to_string(), theme.status_fail_style()),
        },
        ExecutionStatus::Error => ("ERROR".to_string(), theme.status_error_style()),
    }
}

pub(crate) fn standard_screen_layout(
    area: Rect,
    header_height: u16,
    footer_height: u16,
) -> [Rect; 3] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(3),
            Constraint::Length(footer_height),
        ])
        .split(area);

    [chunks[0], chunks[1], chunks[2]]
}

pub(crate) fn horizontal_split(area: Rect, left_percent: u16) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_percent),
            Constraint::Percentage(100 - left_percent),
        ])
        .split(area);

    [chunks[0], chunks[1]]
}

/// Per-state color used by the History list and the Dashboards charts.
/// Kept here so the two views never disagree on the palette.
pub(crate) fn state_style(_theme: &Theme, state: RunState) -> Style {
    match state {
        RunState::Queued => Style::default().fg(Color::Gray),
        RunState::Running => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        RunState::Completed => Style::default().fg(Color::Green),
        RunState::Failed => Style::default().fg(Color::Red),
        RunState::Cancelled => Style::default().fg(Color::Yellow),
        RunState::TimedOut => Style::default().fg(Color::Magenta),
        RunState::DeadLetter => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

/// Bare color companion to [`state_style`]. Used by chart widgets that
/// want only the foreground color (BarChart bars, Canvas slices) and
/// apply their own modifiers.
pub(crate) fn state_color(state: RunState) -> Color {
    match state {
        RunState::Queued => Color::Gray,
        RunState::Running => Color::Cyan,
        RunState::Completed => Color::Green,
        RunState::Failed => Color::Red,
        RunState::Cancelled => Color::Yellow,
        RunState::TimedOut => Color::Magenta,
        RunState::DeadLetter => Color::LightRed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::layout::Rect;
    use rstest::rstest;

    #[rstest]
    #[case::queued(RunState::Queued, Color::Gray)]
    #[case::running(RunState::Running, Color::Cyan)]
    #[case::completed(RunState::Completed, Color::Green)]
    #[case::failed(RunState::Failed, Color::Red)]
    #[case::cancelled(RunState::Cancelled, Color::Yellow)]
    #[case::timed_out(RunState::TimedOut, Color::Magenta)]
    #[case::dead_letter(RunState::DeadLetter, Color::LightRed)]
    fn test_state_color(#[case] state: RunState, #[case] expected: Color) {
        assert_eq!(state_color(state), expected);
    }

    #[test]
    fn test_standard_screen_layout_splits_correctly() {
        let area = Rect::new(0, 0, 80, 24);
        let [header, body, footer] = standard_screen_layout(area, 3, 1);
        assert_eq!(header.height, 3);
        assert_eq!(footer.height, 1);
        assert_eq!(body.height, 24 - 3 - 1);
        assert_eq!(header.width, 80);
    }

    #[test]
    fn test_horizontal_split() {
        let area = Rect::new(0, 0, 100, 24);
        let [left, right] = horizontal_split(area, 40);
        assert_eq!(left.width, 40);
        assert_eq!(right.width, 60);
    }

    #[test]
    fn test_state_style_returns_styled() {
        let theme = Theme::default();
        let style = state_style(&theme, RunState::Completed);
        assert_eq!(style.fg, Some(Color::Green));
    }
}
