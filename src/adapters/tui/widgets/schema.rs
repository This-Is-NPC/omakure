use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::SchemaPreview;

pub(crate) fn render_schema_preview(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    preview: Option<&SchemaPreview>,
    error: Option<&str>,
) {
    let lines = build_lines(preview, error);
    let panel = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, area);
}

fn build_lines(preview: Option<&SchemaPreview>, error: Option<&str>) -> Vec<Line<'static>> {
    if let Some(message) = error {
        return vec![
            Line::from(Span::styled(
                "Failed to load schema.",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(message.to_string()),
        ];
    }

    let preview = match preview {
        Some(preview) => preview,
        None => {
            return vec![Line::from(Span::styled(
                "Select a script to preview its schema.",
                Style::default().fg(Color::Gray),
            ))];
        }
    };

    let mut lines = Vec::new();
    lines.push(Line::from(format!("Name: {}", preview.name)));
    if let Some(description) = preview.description.as_deref() {
        if !description.trim().is_empty() {
            lines.push(Line::from(format!("Description: {}", description.trim())));
        }
    }
    lines.push(Line::from(""));
    if preview.fields.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no fields)",
            Style::default().fg(Color::Gray),
        )));
        return lines;
    }

    lines.push(Line::from(Span::styled(
        format!("Fields: {}", preview.fields.len()),
        Style::default().fg(Color::Cyan),
    )));
    for field in &preview.fields {
        let required_label = if field.required { "required" } else { "optional" };
        let required_style = if field.required {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(vec![
            Span::raw("- "),
            Span::styled(
                field.name.clone(),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ["),
            Span::styled(field.kind.clone(), Style::default().fg(Color::Cyan)),
            Span::raw(", "),
            Span::styled(required_label, required_style),
            Span::raw("]"),
        ]));
        if let Some(prompt) = field.prompt.as_deref() {
            if !prompt.trim().is_empty() {
                lines.push(Line::from(format!("    prompt: {}", prompt.trim())));
            }
        }
    }
    lines
}
