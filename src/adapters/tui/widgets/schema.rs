use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::{QueuePreview, SchemaPreview};

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
    if !preview.tags.is_empty() {
        lines.push(Line::from(format!("Tags: {}", preview.tags.join(", "))));
    }
    lines.push(Line::from(""));
    if preview.fields.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no fields)",
            Style::default().fg(Color::Gray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("Fields: {}", preview.fields.len()),
            Style::default().fg(Color::Cyan),
        )));
        for field in &preview.fields {
            let required_label = if field.required {
                "required"
            } else {
                "optional"
            };
            let required_style = if field.required {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            };
            lines.push(Line::from(vec![
                Span::raw("- "),
                Span::styled(
                    field.name.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
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
    }

    if !preview.outputs.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Outputs: {}", preview.outputs.len()),
            Style::default().fg(Color::Cyan),
        )));
        for output in &preview.outputs {
            lines.push(Line::from(vec![
                Span::raw("- "),
                Span::styled(
                    output.name.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ["),
                Span::styled(output.kind.clone(), Style::default().fg(Color::Cyan)),
                Span::raw("]"),
            ]));
        }
    }

    if let Some(queue) = &preview.queue {
        lines.push(Line::from(""));
        match queue {
            QueuePreview::Matrix { values } => {
                lines.push(Line::from(Span::styled(
                    format!("Queue: Matrix ({})", values.len()),
                    Style::default().fg(Color::Cyan),
                )));
                for entry in values {
                    lines.push(Line::from(vec![
                        Span::raw("- "),
                        Span::styled(
                            entry.name.clone(),
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(": "),
                        Span::raw(entry.values.join(", ")),
                    ]));
                }
            }
            QueuePreview::Cases { cases } => {
                lines.push(Line::from(Span::styled(
                    format!("Queue: Cases ({})", cases.len()),
                    Style::default().fg(Color::Cyan),
                )));
                for (idx, case) in cases.iter().enumerate() {
                    let label = case
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("case {}", idx + 1));
                    lines.push(Line::from(vec![
                        Span::raw("- "),
                        Span::styled(
                            label,
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    for value in &case.values {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(value.name.clone(), Style::default().fg(Color::Yellow)),
                            Span::raw(" = "),
                            Span::raw(value.value.clone()),
                        ]));
                    }
                }
            }
        }
    }

    lines
}
