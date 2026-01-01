use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::{App, SchemaFieldPreview, SchemaPreview};
use super::super::theme;
use crate::search_index::{SearchDetails, SearchResult, SearchStatus};
use super::schema;

pub(crate) fn render_search(frame: &mut Frame, area: Rect, app: &mut App) {
    let outer = Block::default().borders(Borders::ALL).title("Search");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    render_search_input(frame, chunks[0], app);
    render_search_body(frame, chunks[1], app);
    render_search_footer(frame, chunks[2], app);
}

fn render_search_input(frame: &mut Frame, area: Rect, app: &App) {
    let title = match &app.search_status {
        SearchStatus::Indexing => "Search (indexing...)".to_string(),
        SearchStatus::Ready { script_count } => format!("Search ({} scripts)", script_count),
        SearchStatus::Error(_) => "Search (index error)".to_string(),
        SearchStatus::Idle => "Search".to_string(),
    };
    let query_line = if app.search_query.is_empty() {
        Line::from(Span::styled(
            "Type to search...",
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from(app.search_query.clone())
    };
    let input = Paragraph::new(vec![query_line])
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: true });
    frame.render_widget(input, area);
}

fn render_search_body(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.search_results.is_empty() {
        let message = if let Some(err) = &app.search_error {
            format!("Search error: {}", err)
        } else if matches!(app.search_status, SearchStatus::Indexing) {
            "Indexing scripts...".to_string()
        } else {
            "No scripts found for this search.".to_string()
        };
        let empty = Paragraph::new(message)
            .block(Block::default().borders(Borders::ALL).title("Results"))
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_search_results(frame, body_chunks[0], app);
    render_search_schema(frame, body_chunks[1], app);
}

fn render_search_results(frame: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = app
        .search_results
        .iter()
        .map(|result| ListItem::new(result_label(result)))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Results"))
        .highlight_style(theme::selection_style())
        .highlight_symbol(theme::selection_symbol_str());

    frame.render_stateful_widget(list, area, &mut app.search_state);
}

fn render_search_schema(frame: &mut Frame, area: Rect, app: &App) {
    let selected = app.search_results.get(app.search_state.selected().unwrap_or(0));
    let title = schema_title(selected);
    let (preview, error) = match (app.search_details.as_ref(), selected) {
        (Some(details), _) => (
            Some(build_schema_preview_from_details(details)),
            details.schema_error.as_deref(),
        ),
        (None, Some(result)) => (
            Some(build_schema_preview_from_result(result)),
            result.schema_error.as_deref(),
        ),
        _ => (None, None),
    };
    schema::render_schema_preview(frame, area, &title, preview.as_ref(), error);
}

fn render_search_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hint = match &app.search_status {
        SearchStatus::Indexing => "Type to search, Enter open, Esc back. Indexing in background.",
        SearchStatus::Error(_) => "Type to search, Enter open, Esc back. Index error.",
        _ => "Type to search, Enter open, Esc back",
    };
    let footer = Paragraph::new(hint).style(Style::default().fg(Color::Gray));
    frame.render_widget(footer, area);
}

fn result_label(result: &SearchResult) -> String {
    let path = result.script_path.to_string_lossy();
    if result.display_name == path {
        path.to_string()
    } else {
        format!("{} ({})", result.display_name, path)
    }
}

fn schema_title(selected: Option<&SearchResult>) -> String {
    let Some(selected) = selected else {
        return "Schema".to_string();
    };
    let name = selected
        .script_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Schema");
    format!("Schema: {}", name)
}

fn build_schema_preview_from_details(details: &SearchDetails) -> SchemaPreview {
    let fields = details
        .fields
        .iter()
        .map(|field| SchemaFieldPreview {
            name: field.name.clone(),
            prompt: field.prompt.clone(),
            kind: field.kind.clone(),
            required: field.required,
        })
        .collect();
    SchemaPreview {
        name: details.display_name.clone(),
        description: details.description.clone(),
        tags: details.tags.clone(),
        fields,
    }
}

fn build_schema_preview_from_result(result: &SearchResult) -> SchemaPreview {
    SchemaPreview {
        name: result.display_name.clone(),
        description: result.description.clone(),
        tags: result.tags.clone(),
        fields: Vec::new(),
    }
}
