use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::App;
use super::super::theme;

pub(crate) fn render_field_input(frame: &mut Frame, area: Rect, app: &mut App) {
    let script_name = app
        .selected_script
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>");

    let label_style = Style::default().fg(Color::Gray);
    let value_style = Style::default();
    let mut header_lines = vec![
        Line::from(vec![
            Span::styled("Script: ", label_style),
            Span::styled(script_name, value_style),
        ]),
        Line::from(vec![
            Span::styled("Schema: ", label_style),
            Span::styled(app.schema_name.as_deref().unwrap_or("-"), value_style),
        ]),
        Line::from(vec![
            Span::styled("Description: ", label_style),
            Span::raw(app.schema_description.as_deref().unwrap_or("-")),
        ]),
    ];
    if let Some(message) = &app.error {
        header_lines.push(Line::from(Span::styled(
            format!("Error: {}", message),
            Style::default().fg(Color::Red),
        )));
    }
    let header_height = header_lines.len() as u16 + 2;
    let header = Paragraph::new(header_lines)
        .block(Block::default().borders(Borders::ALL).title("Schema"))
        .wrap(Wrap { trim: true });

    let footer = Paragraph::new("Tab/Shift+Tab to move, Enter to run, Ctrl+B back, Esc quit")
        .style(Style::default().fg(Color::Gray));

    let footer_height = 1u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(3),
            Constraint::Length(footer_height),
        ])
        .split(area);

    frame.render_widget(header, chunks[0]);
    render_field_boxes(frame, chunks[1], app);
    frame.render_widget(footer, chunks[2]);
}

fn render_field_boxes(frame: &mut Frame, area: Rect, app: &App) {
    let outer = Block::default().borders(Borders::ALL).title("Fields");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    if app.fields.is_empty() {
        let empty = Paragraph::new("No fields found.")
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, inner);
        return;
    }

    let box_height = 4u16;
    let max_boxes = (inner.height / box_height).max(1) as usize;
    let total = app.fields.len();
    let mut start = if app.field_index >= max_boxes {
        app.field_index + 1 - max_boxes
    } else {
        0
    };
    if total > max_boxes {
        start = start.min(total - max_boxes);
    }
    let end = (start + max_boxes).min(total);

    let mut y = inner.y;
    for idx in start..end {
        let field = &app.fields[idx];
        let required = field.required.unwrap_or(false);
        let required_label = if required { "required" } else { "optional" };
        let title = format!("{} ({}, {})", field.name, field.kind, required_label);
        let is_selected = idx == app.field_index;
        let border_style = if is_selected {
            Style::default()
                .fg(theme::brand_accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let value = app
            .field_inputs
            .get(idx)
            .map(String::as_str)
            .unwrap_or("");
        let value_text = if value.trim().is_empty() {
            field
                .default
                .as_deref()
                .map(|default| format!("<default: {}>", default))
                .unwrap_or_else(|| "<empty>".to_string())
        } else {
            value.to_string()
        };
        let prompt = field.prompt.as_deref().unwrap_or(&field.name);
        let value_style = if is_selected {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };

        let lines = vec![
            Line::from(vec![
                Span::styled("Prompt: ", Style::default().fg(Color::Gray)),
                Span::raw(prompt),
            ]),
            Line::from(vec![
                Span::styled("Value: ", Style::default().fg(Color::Gray)),
                Span::styled(value_text, value_style),
            ]),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(border_style);
        let rect = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: box_height,
        };
        let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
        frame.render_widget(paragraph, rect);
        y = y.saturating_add(box_height);
    }
}
