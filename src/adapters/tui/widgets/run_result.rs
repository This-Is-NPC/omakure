use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::path::PathBuf;

use super::super::app::{App, ExecutionStatus};
use super::super::theme::Theme;
use super::common::status_label_and_style;
use super::history::format_run_output;

pub(crate) fn render_run_result(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let lines = render_lines(app, theme);
    let view_height = chunks[0].height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if max_scroll == 0 {
        app.run_output_scroll = 0;
    } else if app.run_output_scroll as usize > max_scroll {
        app.run_output_scroll = max_scroll.min(u16::MAX as usize) as u16;
    }

    let output = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Last run output"),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.run_output_scroll, 0));
    frame.render_widget(output, chunks[0]);

    let footer = Paragraph::new("Up/Down to scroll, PgUp/PgDn, Enter/Esc to return, h for history")
        .style(theme.text_secondary());
    frame.render_widget(footer, chunks[1]);
}

fn render_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let entry = match app.history.entries.first() {
        Some(entry) => entry,
        None => {
            lines.push(Line::from("No script output yet."));
            return lines;
        }
    };

    let name = app.display_path(&PathBuf::from(&entry.script_path));
    let args = match serde_json::from_str::<Vec<String>>(&entry.args_json) {
        Ok(args) if args.is_empty() => "-".to_string(),
        Ok(args) => args.join(" "),
        Err(_) => entry.args_json.clone(),
    };
    let status = ExecutionStatus::from_run(entry);
    let (status_label, status_style) = status_label_and_style(&status, theme);
    lines.push(Line::from(format!("Script: {}", name)));
    lines.push(Line::from(format!("Args: {}", args)));
    lines.push(Line::from(vec![
        Span::raw("Status: "),
        Span::styled(status_label, status_style),
    ]));
    lines.push(Line::from(""));
    let output = format_run_output(entry);
    if output.trim().is_empty() {
        lines.push(Line::from("(no output)"));
    } else {
        lines.extend(output.lines().map(|line| Line::from(line.to_string())));
    }
    lines
}
