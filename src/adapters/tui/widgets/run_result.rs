use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::{App, ExecutionStatus};
use crate::history;

pub(crate) fn render_run_result(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let lines = render_lines(app);
    let view_height = chunks[0].height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if max_scroll == 0 {
        app.run_output_scroll = 0;
    } else if app.run_output_scroll as usize > max_scroll {
        app.run_output_scroll = max_scroll.min(u16::MAX as usize) as u16;
    }

    let output = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Last run output"))
        .wrap(Wrap { trim: false })
        .scroll((app.run_output_scroll, 0));
    frame.render_widget(output, chunks[0]);

    let footer = Paragraph::new("Up/Down to scroll, PgUp/PgDn, Enter/Esc to return, h for history")
        .style(Style::default().fg(Color::Gray));
    frame.render_widget(footer, chunks[1]);
}

fn render_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let entry = match app.history.first() {
        Some(entry) => entry,
        None => {
            lines.push(Line::from("No script output yet."));
            return lines;
        }
    };

    let name = app.display_path(&entry.script);
    let args = if entry.args.is_empty() {
        "-".to_string()
    } else {
        entry.args.join(" ")
    };
    let status = ExecutionStatus::from_history(entry);
    let (status_label, status_style) = status_label_and_style(&status);
    lines.push(Line::from(format!("Script: {}", name)));
    lines.push(Line::from(format!("Args: {}", args)));
    lines.push(Line::from(vec![
        Span::raw("Status: "),
        Span::styled(status_label, status_style),
    ]));
    lines.push(Line::from(""));
    let output = history::format_output(entry);
    if output.trim().is_empty() {
        lines.push(Line::from("(no output)"));
    } else {
        lines.extend(output.lines().map(|line| Line::from(line.to_string())));
    }
    lines
}

fn status_label_and_style(status: &ExecutionStatus) -> (String, Style) {
    match status {
        ExecutionStatus::Success => ("OK".to_string(), Style::default().fg(Color::Green)),
        ExecutionStatus::Failed(code) => match code {
            Some(code) => (
                format!("FAIL ({})", code),
                Style::default().fg(Color::Red),
            ),
            None => ("FAIL".to_string(), Style::default().fg(Color::Red)),
        },
        ExecutionStatus::Error => ("ERROR".to_string(), Style::default().fg(Color::Yellow)),
    }
}
