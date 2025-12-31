use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use super::super::app::{App, ExecutionStatus, HistoryFocus};
use super::super::theme;
use crate::history;

pub(crate) fn render_history(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let list_width = history_list_width(chunks[0].width, app);
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(list_width), Constraint::Min(10)])
        .split(chunks[0]);

    render_history_list(frame, body_chunks[0], app);
    render_history_output(frame, body_chunks[1], app);

    let footer_text = match app.history_focus {
        HistoryFocus::List => "Up/Down to select, Enter to view output, Esc/q to go back",
        HistoryFocus::Output => "Up/Down to scroll, PgUp/PgDn, Esc to return, q to go back",
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::Gray));
    frame.render_widget(footer, chunks[1]);
}

fn render_history_list(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.history.is_empty() {
        let empty = Paragraph::new("No executions yet.")
            .block(Block::default().borders(Borders::ALL).title("History"))
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let rows: Vec<Row> = app
        .history
        .iter()
        .map(|entry| {
            let name = app.display_path(&entry.script);
            let date = history::format_timestamp(entry.timestamp);
            let status = ExecutionStatus::from_history(entry);
            let (status_label, status_style) = status_label_and_style(&status);
            Row::new(vec![
                Cell::from(Span::styled(status_label, status_style)),
                Cell::from(Span::raw(date)),
                Cell::from(Span::raw(name)),
            ])
        })
        .collect();

    let header = Row::new(vec![
        Cell::from(Span::styled("Status", Style::default().fg(Color::Gray))),
        Cell::from(Span::styled("Date", Style::default().fg(Color::Gray))),
        Cell::from(Span::styled("Script", Style::default().fg(Color::Gray))),
    ]);
    let highlight_style = match app.history_focus {
        HistoryFocus::List => theme::selection_style(),
        HistoryFocus::Output => Style::default().fg(Color::DarkGray),
    };
    let highlight_symbol = if app.history_focus == HistoryFocus::List {
        theme::selection_symbol()
    } else {
        Span::styled("> ", highlight_style)
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(HISTORY_STATUS_WIDTH),
            Constraint::Length(HISTORY_DATE_WIDTH),
            Constraint::Min(HISTORY_MIN_SCRIPT_WIDTH),
        ],
    )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("History"))
        .highlight_style(highlight_style)
        .highlight_symbol(highlight_symbol);

    frame.render_stateful_widget(table, area, &mut app.history_state);
}

fn render_history_output(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut lines = Vec::new();
    if let Some(entry) = app.current_history_entry() {
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
    } else {
        lines.push(Line::from("No history selected."));
    }

    let view_height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if max_scroll == 0 {
        app.run_output_scroll = 0;
    } else if app.run_output_scroll as usize > max_scroll {
        app.run_output_scroll = max_scroll.min(u16::MAX as usize) as u16;
    }

    let mut block = Block::default().borders(Borders::ALL).title("Output");
    if app.history_focus == HistoryFocus::Output {
        let border_style = theme::selection_border_style();
        block = block.border_style(border_style).title_style(border_style);
    }

    let output = Paragraph::new(lines)
        .block(block)
        .style(Style::default())
        .wrap(Wrap { trim: false })
        .scroll((app.run_output_scroll, 0));
    frame.render_widget(output, area);
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

const HISTORY_STATUS_WIDTH: u16 = 10;
const HISTORY_DATE_WIDTH: u16 = 16;
const HISTORY_MIN_SCRIPT_WIDTH: u16 = 10;
const HISTORY_COLUMN_SPACING: u16 = 1;
const HISTORY_HIGHLIGHT_WIDTH: u16 = 2;
const HISTORY_BORDER_WIDTH: u16 = 2;
const HISTORY_MIN_OUTPUT_WIDTH: u16 = 30;

fn history_list_width(total_width: u16, app: &App) -> u16 {
    let max_script = app
        .history
        .iter()
        .map(|entry| app.display_path(&entry.script).len() as u16)
        .max()
        .unwrap_or(0)
        .max(HISTORY_MIN_SCRIPT_WIDTH);

    let content_width = HISTORY_STATUS_WIDTH
        + HISTORY_DATE_WIDTH
        + max_script
        + HISTORY_COLUMN_SPACING * 2;
    let desired = content_width + HISTORY_BORDER_WIDTH + HISTORY_HIGHLIGHT_WIDTH;
    let min_output = HISTORY_MIN_OUTPUT_WIDTH
        .min(total_width.saturating_sub(10).max(1));
    let max_list = total_width.saturating_sub(min_output);
    desired.min(max_list).max(1)
}
