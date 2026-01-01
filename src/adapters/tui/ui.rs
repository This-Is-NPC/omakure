use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::app::{App, Screen};
use super::theme::{BRAND_GRADIENT_END, BRAND_GRADIENT_START};
use super::widgets::{
    environment, error as error_widget, field_input, history, loading as loading_widget, run_result,
    running, schema, scripts, search,
};

pub(crate) fn render_ui(frame: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::ScriptSelect => render_script_select(frame, app),
        Screen::Search => search::render_search(frame, frame.size(), app),
        Screen::FieldInput => field_input::render_field_input(frame, frame.size(), app),
        Screen::History => history::render_history(frame, frame.size(), app),
        Screen::Running => running::render_running(frame, frame.size(), app),
        Screen::RunResult => run_result::render_run_result(frame, frame.size(), app),
        Screen::Error => render_error(frame, app),
    }
}

pub(crate) fn render_loading(frame: &mut Frame) {
    loading_widget::render_loading(frame, frame.size());
}

fn render_script_select(frame: &mut Frame, app: &mut App) {
    let (info_title, info_lines) = environment::status_info(
        &app.workspace,
        app.widget.as_ref(),
        app.widget_error.as_deref(),
    );
    let info_height = info_lines.len() as u16 + 2;

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(omakure_title_line());
    let inner = outer.inner(frame.size());
    frame.render_widget(outer, frame.size());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(info_height),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    environment::render_environment(frame, chunks[0], &info_title, info_lines);
    let entries_block = Block::default()
        .borders(Borders::ALL)
        .title("Workspace Entries");
    let entries_area = entries_block.inner(chunks[1]);
    frame.render_widget(entries_block, chunks[1]);

    let show_schema = matches!(
        app.selected_entry(),
        Some(entry) if entry.kind == crate::ports::WorkspaceEntryKind::Script
    );

    if show_schema {
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(entries_area);

        scripts::render_scripts(
            frame,
            body_chunks[0],
            &app.workspace,
            &app.current_dir,
            &app.entries,
            &mut app.list_state,
        );
        let schema_title = schema_title(app);
        schema::render_schema_preview(
            frame,
            body_chunks[1],
            &schema_title,
            app.schema_preview.as_ref(),
            app.schema_preview_error.as_deref(),
        );
    } else {
        scripts::render_scripts(
            frame,
            entries_area,
            &app.workspace,
            &app.current_dir,
            &app.entries,
            &mut app.list_state,
        );
    }

    let mut footer_text = if app.entries.is_empty() {
        "Folder is empty. r refresh, h history, Ctrl+S search, q quit".to_string()
    } else {
        "Up/Down move, Enter open/run, r refresh, h history, Ctrl+S search, q quit".to_string()
    };
    if app.current_dir != app.workspace.root() {
        if app.entries.is_empty() {
            footer_text =
                "Folder is empty. Backspace up, r refresh, h history, Ctrl+S search, q quit"
                    .to_string();
        } else {
            footer_text =
                "Up/Down move, Enter open/run, Backspace up, r refresh, h history, Ctrl+S search, q quit"
                    .to_string();
        }
    }
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::Gray));
    frame.render_widget(footer, chunks[2]);
}

fn render_error(frame: &mut Frame, app: &mut App) {
    let message = app
        .error
        .as_deref()
        .unwrap_or("Unknown error while loading schema");
    error_widget::render_error(frame, frame.size(), message);
}

fn schema_title(app: &App) -> String {
    let entry = match app.selected_entry() {
        Some(entry) => entry,
        None => return "Schema".to_string(),
    };
    if entry.kind != crate::ports::WorkspaceEntryKind::Script {
        return "Schema".to_string();
    }
    let name = entry
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Schema");
    format!("Schema: {}", name)
}

fn omakure_title_line() -> Line<'static> {
    gradient_line("omakure", BRAND_GRADIENT_START, BRAND_GRADIENT_END)
}

fn gradient_line(text: &str, start: (u8, u8, u8), end: (u8, u8, u8)) -> Line<'static> {
    let len = text.chars().count().max(1);
    let spans = text
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            let t = if len <= 1 {
                0.0
            } else {
                idx as f32 / (len - 1) as f32
            };
            let color = lerp_color(start, end, t);
            Span::styled(ch.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD))
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn lerp_color(start: (u8, u8, u8), end: (u8, u8, u8), t: f32) -> Color {
    let lerp = |a, b| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::Rgb(lerp(start.0, end.0), lerp(start.1, end.1), lerp(start.2, end.2))
}
