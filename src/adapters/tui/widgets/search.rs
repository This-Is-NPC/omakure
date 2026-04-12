use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::{App, SchemaFieldPreview, SchemaPreview};
use super::super::theme::{self, Theme};
use super::common::{horizontal_split, standard_screen_layout};
use super::schema;
use super::spinner::{spinner_span, SpinnerKind};
use crate::search_index::{SearchDetails, SearchResult, SearchStatus};

pub(crate) fn render_search(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let outer = Block::default().borders(Borders::ALL).title("Search");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let chunks = standard_screen_layout(inner, 3, 2);

    render_search_input(frame, chunks[0], app, theme);
    render_search_body(frame, chunks[1], app, theme);
    render_search_footer(frame, chunks[2], app, theme);
}

fn render_search_input(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let title_line: Line<'static> = match &app.search.status {
        SearchStatus::Indexing => Line::from(vec![
            spinner_span(SpinnerKind::Scan, app.tick, theme),
            Span::raw("Search (indexing...)"),
        ]),
        SearchStatus::Ready { script_count } => {
            Line::from(format!("Search ({} scripts)", script_count))
        }
        SearchStatus::Error(_) => Line::from("Search (index error)".to_string()),
        SearchStatus::Idle => Line::from("Search".to_string()),
    };
    let query_line = if app.search.query.is_empty() {
        Line::from(Span::styled("Type to search...", theme.text_muted()))
    } else {
        Line::from(app.search.query.clone())
    };
    let input = Paragraph::new(vec![query_line])
        .block(Block::default().borders(Borders::ALL).title(title_line))
        .wrap(Wrap { trim: true });
    frame.render_widget(input, area);
}

fn render_search_body(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if app.search.results.is_empty() {
        let lines: Vec<Line<'static>> = if let Some(err) = &app.search.error {
            vec![Line::from(format!("Search error: {}", err))]
        } else if matches!(app.search.status, SearchStatus::Indexing) {
            vec![Line::from(vec![
                spinner_span(SpinnerKind::Scan, app.tick, theme),
                Span::raw("Indexing scripts..."),
            ])]
        } else {
            vec![Line::from("No scripts found for this search.")]
        };
        let empty = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Results"))
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let body_chunks = horizontal_split(area, 50);

    render_search_results(frame, body_chunks[0], app, theme);
    render_search_schema(frame, body_chunks[1], app, theme);
}

fn render_search_results(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let items: Vec<ListItem> = app
        .search
        .results
        .iter()
        .map(|result| ListItem::new(result_label(result)))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Results"))
        .highlight_style(theme.selection_style())
        .highlight_symbol(theme::selection_symbol_str());

    frame.render_stateful_widget(list, area, &mut app.search.list_state);
}

fn render_search_schema(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let selected = app
        .search
        .results
        .get(app.search.list_state.selected().unwrap_or(0));
    let title = schema_title(selected);
    let (preview, error) = match (app.search.details.as_ref(), selected) {
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
    schema::render_schema_preview(frame, area, &title, preview.as_ref(), error, theme);
}

fn render_search_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let hint = match &app.search.status {
        SearchStatus::Indexing => {
            "Type to search, Enter open, Alt+E envs, Esc back. Indexing in background."
        }
        SearchStatus::Error(_) => "Type to search, Enter open, Alt+E envs, Esc back. Index error.",
        _ => "Type to search, Enter open, Alt+E envs, Esc back",
    };
    let footer = Paragraph::new(hint).style(theme.text_secondary());
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
        outputs: Vec::new(),
        queue: None,
    }
}

fn build_schema_preview_from_result(result: &SearchResult) -> SchemaPreview {
    SchemaPreview {
        name: result.display_name.clone(),
        description: result.description.clone(),
        tags: result.tags.clone(),
        fields: Vec::new(),
        outputs: Vec::new(),
        queue: None,
    }
}
