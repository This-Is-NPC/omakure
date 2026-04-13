use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::{QueuePreview, SchemaPreview};
use super::super::theme::Theme;

pub(crate) fn render_schema_preview(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    preview: Option<&SchemaPreview>,
    error: Option<&str>,
    theme: &Theme,
) {
    let lines = build_lines(preview, error, theme);
    let panel = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, area);
}

fn build_lines(
    preview: Option<&SchemaPreview>,
    error: Option<&str>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if let Some(message) = error {
        return vec![
            Line::from(Span::styled(
                "Failed to load schema.",
                Style::default()
                    .fg(theme.semantic.error.color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(message.to_string()),
        ];
    }

    let preview = match preview {
        Some(preview) => preview,
        None => {
            return vec![Line::from(Span::styled(
                "Select a script to preview its schema.",
                theme.text_muted(),
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
        lines.push(Line::from(Span::styled("(no fields)", theme.text_muted())));
    } else {
        lines.push(Line::from(Span::styled(
            format!("Fields: {}", preview.fields.len()),
            Style::default().fg(theme.semantic.info.color()),
        )));
        for field in &preview.fields {
            let required_label = if field.required {
                "required"
            } else {
                "optional"
            };
            let required_style = if field.required {
                Style::default().fg(theme.semantic.error.color())
            } else {
                Style::default().fg(theme.semantic.success.color())
            };
            lines.push(Line::from(vec![
                Span::raw("- "),
                Span::styled(
                    field.name.clone(),
                    Style::default()
                        .fg(theme.semantic.warning.color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ["),
                Span::styled(
                    field.kind.clone(),
                    Style::default().fg(theme.semantic.info.color()),
                ),
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
            Style::default().fg(theme.semantic.info.color()),
        )));
        for output in &preview.outputs {
            lines.push(Line::from(vec![
                Span::raw("- "),
                Span::styled(
                    output.name.clone(),
                    Style::default()
                        .fg(theme.semantic.warning.color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ["),
                Span::styled(
                    output.kind.clone(),
                    Style::default().fg(theme.semantic.info.color()),
                ),
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
                    Style::default().fg(theme.semantic.info.color()),
                )));
                for entry in values {
                    lines.push(Line::from(vec![
                        Span::raw("- "),
                        Span::styled(
                            entry.name.clone(),
                            Style::default()
                                .fg(theme.semantic.warning.color())
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
                    Style::default().fg(theme.semantic.info.color()),
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
                                .fg(theme.semantic.warning.color())
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    for value in &case.values {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(
                                value.name.clone(),
                                Style::default().fg(theme.semantic.warning.color()),
                            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::tui::app::SchemaFieldPreview;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn sample_preview() -> SchemaPreview {
        SchemaPreview {
            name: "Deploy App".to_string(),
            description: Some("Deploy to production".to_string()),
            tags: vec!["ops".to_string(), "deploy".to_string()],
            fields: vec![
                SchemaFieldPreview {
                    name: "target".to_string(),
                    prompt: Some("Target environment".to_string()),
                    kind: "string".to_string(),
                    required: true,
                },
                SchemaFieldPreview {
                    name: "dry_run".to_string(),
                    prompt: None,
                    kind: "boolean".to_string(),
                    required: false,
                },
            ],
            outputs: vec![],
            queue: None,
        }
    }

    #[test]
    fn snapshot_schema_with_fields() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        let preview = sample_preview();
        terminal
            .draw(|f| {
                render_schema_preview(f, f.size(), "Schema", Some(&preview), None, &theme);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_schema_no_preview() {
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_schema_preview(f, f.size(), "Schema", None, None, &theme);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_schema_error() {
        let backend = TestBackend::new(60, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_schema_preview(
                    f,
                    f.size(),
                    "Schema",
                    None,
                    Some("Invalid JSON at line 5"),
                    &theme,
                );
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn build_lines_with_outputs_and_matrix_queue() {
        use crate::adapters::tui::app::{MatrixPreview, SchemaOutputPreview};
        let theme = Theme::default();
        let preview = SchemaPreview {
            name: "Build".to_string(),
            description: Some(" trimmed ".to_string()),
            tags: vec!["t1".into()],
            fields: vec![SchemaFieldPreview {
                name: "x".into(),
                prompt: Some("  prompt text  ".into()),
                kind: "string".into(),
                required: true,
            }],
            outputs: vec![SchemaOutputPreview {
                name: "url".into(),
                kind: "string".into(),
            }],
            queue: Some(QueuePreview::Matrix {
                values: vec![MatrixPreview {
                    name: "region".into(),
                    values: vec!["us".into(), "eu".into()],
                }],
            }),
        };
        let lines = build_lines(Some(&preview), None, &theme);
        let joined = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Outputs: 1"));
        assert!(joined.contains("Queue: Matrix"));
        assert!(joined.contains("region"));
        assert!(joined.contains("us"));
        assert!(joined.contains("prompt text"));
    }

    #[test]
    fn build_lines_with_cases_queue() {
        use crate::adapters::tui::app::{QueueCasePreview, QueueCaseValuePreview};
        let theme = Theme::default();
        let preview = SchemaPreview {
            name: "Run".to_string(),
            description: None,
            tags: vec![],
            fields: vec![],
            outputs: vec![],
            queue: Some(QueuePreview::Cases {
                cases: vec![
                    QueueCasePreview {
                        name: Some("named-case".into()),
                        values: vec![QueueCaseValuePreview {
                            name: "k".into(),
                            value: "v".into(),
                        }],
                    },
                    QueueCasePreview {
                        name: None,
                        values: vec![],
                    },
                ],
            }),
        };
        let lines = build_lines(Some(&preview), None, &theme);
        let joined = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Queue: Cases (2)"));
        assert!(joined.contains("named-case"));
        assert!(joined.contains("case 2"));
        assert!(joined.contains("k = v"));
    }

    #[test]
    fn snapshot_schema_no_fields() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::default();
        let preview = SchemaPreview {
            name: "Simple".to_string(),
            description: None,
            tags: vec![],
            fields: vec![],
            outputs: vec![],
            queue: None,
        };
        terminal
            .draw(|f| {
                render_schema_preview(f, f.size(), "Schema", Some(&preview), None, &theme);
            })
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
