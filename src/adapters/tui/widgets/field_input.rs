use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::App;
use super::super::theme::Theme;
use super::common::standard_screen_layout;

pub(crate) fn render_field_input(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let script_name = app
        .field_input
        .selected_script
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>");

    let label_style = theme.text_secondary();
    let value_style = Style::default();
    let mut header_lines = vec![
        Line::from(vec![
            Span::styled("Script: ", label_style),
            Span::styled(script_name, value_style),
        ]),
        Line::from(vec![
            Span::styled("Schema: ", label_style),
            Span::styled(
                app.field_input.schema_name.as_deref().unwrap_or("-"),
                value_style,
            ),
        ]),
        Line::from(vec![
            Span::styled("Description: ", label_style),
            Span::raw(app.field_input.schema_description.as_deref().unwrap_or("-")),
        ]),
    ];
    if let Some(message) = &app.field_input.error {
        header_lines.push(Line::from(Span::styled(
            format!("Error: {}", message),
            Style::default().fg(theme.semantic.error.color()),
        )));
    }
    let header_height = header_lines.len() as u16 + 2;
    let header = Paragraph::new(header_lines)
        .block(Block::default().borders(Borders::ALL).title("Schema"))
        .wrap(Wrap { trim: true });

    let footer = Paragraph::new("Tab/Shift+Tab to move, Enter to run, Ctrl+B back, Esc quit")
        .style(theme.text_secondary());

    let footer_height = 1u16;
    let chunks = standard_screen_layout(area, header_height, footer_height);

    frame.render_widget(header, chunks[0]);
    render_field_boxes(frame, chunks[1], app, theme);
    frame.render_widget(footer, chunks[2]);
}

fn render_field_boxes(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let outer = Block::default().borders(Borders::ALL).title("Fields");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    if app.field_input.fields.is_empty() {
        let empty = Paragraph::new("No fields found.").wrap(Wrap { trim: true });
        frame.render_widget(empty, inner);
        return;
    }

    let box_height = 4u16;
    let max_boxes = (inner.height / box_height).max(1) as usize;
    let total = app.field_input.fields.len();
    let mut start = if app.field_input.field_index >= max_boxes {
        app.field_input.field_index + 1 - max_boxes
    } else {
        0
    };
    if total > max_boxes {
        start = start.min(total - max_boxes);
    }
    let end = (start + max_boxes).min(total);

    let mut y = inner.y;
    for idx in start..end {
        let field = &app.field_input.fields[idx];
        let required = field.required.unwrap_or(false);
        let required_label = if required { "required" } else { "optional" };
        let title = format!("{} ({}, {})", field.name, field.kind, required_label);
        let is_selected = idx == app.field_input.field_index;
        let border_style = if is_selected {
            Style::default()
                .fg(theme.ui.border_active.color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.ui.border_inactive.color())
        };
        let value = app
            .field_input
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
            Style::default().fg(theme.semantic.info.color())
        } else {
            theme.text_secondary()
        };

        let lines = vec![
            Line::from(vec![
                Span::styled("Prompt: ", theme.text_secondary()),
                Span::raw(prompt),
            ]),
            Line::from(vec![
                Span::styled("Value: ", theme.text_secondary()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::domain::Field;
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn snapshot_field_input_with_fields() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.selected_script = Some(PathBuf::from("/scripts/deploy.sh"));
        app.field_input.schema_name = Some("Deploy".to_string());
        app.field_input.fields = vec![
            Field {
                name: "target".to_string(),
                prompt: Some("Target environment".to_string()),
                kind: "string".to_string(),
                order: 0,
                required: Some(true),
                default: None,
                choices: None,
                arg: None,
            },
            Field {
                name: "dry_run".to_string(),
                prompt: Some("Dry run?".to_string()),
                kind: "boolean".to_string(),
                order: 1,
                required: None,
                default: Some("false".to_string()),
                choices: None,
                arg: None,
            },
        ];
        app.field_input.field_inputs = vec!["prod".to_string(), String::new()];
        app.field_input.field_index = 0;

        let theme = app.theme.clone();
        let backend = TestBackend::new(60, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_field_input(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn snapshot_field_input_with_error() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.error = Some("Validation failed: target is required".to_string());

        let theme = app.theme.clone();
        let backend = TestBackend::new(60, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_field_input(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
