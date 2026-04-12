use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::super::app::App;
use super::super::theme::{self, Theme};
use super::common::{horizontal_split, standard_screen_layout};

fn build_preview_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    if let Some(err) = app.environment.preview_error.as_deref() {
        return vec![
            Line::from(Span::styled(
                "Failed to load env file.",
                Style::default().fg(theme.semantic.error.color()),
            )),
            Line::from(err.to_string()),
        ];
    }

    if app.environment.entries.is_empty() {
        return vec![Line::from(Span::styled(
            "No environment files found.",
            theme.text_muted(),
        ))];
    }

    if app.environment.preview_lines.is_empty() {
        return vec![Line::from(Span::styled(
            "Select a file to preview.",
            theme.text_muted(),
        ))];
    }

    app.environment.preview_lines.clone()
}

pub(crate) fn render_envs(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let outer = Block::default().borders(Borders::ALL).title("Environments");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let active_name = app
        .environment
        .config
        .as_ref()
        .and_then(|config| config.active.as_deref())
        .unwrap_or("<none>");

    let envs_dir = app
        .environment
        .config
        .as_ref()
        .map(|config| config.envs_dir.display().to_string())
        .unwrap_or_else(|| app.workspace.envs_dir().display().to_string());
    let mut info_lines = vec![
        Line::from(format!("Dir: {}", envs_dir)),
        Line::from(format!("Active: {}", active_name)),
    ];
    let defaults_count = app
        .environment
        .config
        .as_ref()
        .map(|config| config.defaults.len())
        .unwrap_or(0);
    info_lines.push(Line::from(format!("Defaults: {}", defaults_count)));
    if let Some(err) = &app.environment.error {
        info_lines.push(Line::from(vec![
            Span::styled("Error: ", Style::default().fg(theme.semantic.error.color())),
            Span::raw(err),
        ]));
    }
    let info_height = info_lines.len() as u16 + 2;

    let chunks = standard_screen_layout(inner, info_height, 2);

    let info = Paragraph::new(info_lines)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: true });
    frame.render_widget(info, chunks[0]);

    let files_chunks = horizontal_split(chunks[1], 50);

    if app.environment.entries.is_empty() {
        let empty = Paragraph::new("No environment files found.")
            .block(Block::default().borders(Borders::ALL).title("Files"))
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, files_chunks[0]);
    } else {
        let items: Vec<ListItem> = app
            .environment
            .entries
            .iter()
            .map(|entry| {
                let active = app
                    .environment
                    .config
                    .as_ref()
                    .and_then(|config| config.active.as_deref())
                    .map(|name| name == entry.name)
                    .unwrap_or(false);
                let label = if active {
                    format!("* {}", entry.name)
                } else {
                    format!("  {}", entry.name)
                };
                ListItem::new(Line::from(label))
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Files"))
            .highlight_style(theme.selection_style())
            .highlight_symbol(theme::selection_symbol_str());
        frame.render_stateful_widget(list, files_chunks[0], &mut app.environment.list_state);
    }

    let preview_lines = build_preview_lines(app, theme);
    let preview = Paragraph::new(preview_lines)
        .block(Block::default().borders(Borders::ALL).title("Preview"))
        .wrap(Wrap { trim: false })
        .scroll((app.environment.preview_scroll, 0));
    frame.render_widget(preview, files_chunks[1]);

    let footer = Paragraph::new(
        "Up/Down move, PgUp/PgDn scroll, Enter activate, d deactivate, r reload, Esc/q back",
    )
    .style(theme.text_secondary());
    frame.render_widget(footer, chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::ports::EnvFile;
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn snapshot_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("omakure_envs_snapshot_{label}"))
    }

    #[test]
    fn snapshot_render_envs_with_files() {
        let root = snapshot_root("with_files");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = FsWorkspaceRepository::new(&root);
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(root.clone());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.screen = crate::adapters::tui::app::Screen::Environments;
        app.environment.entries = vec![
            EnvFile {
                name: "dev.conf".to_string(),
            },
            EnvFile {
                name: "prod.conf".to_string(),
            },
        ];
        app.environment.list_state.select(Some(0));
        app.environment.preview_lines = vec![Line::from("HOST=localhost"), Line::from("PORT=3000")];
        let theme = app.theme.clone();

        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_envs(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_render_envs_empty() {
        let root = snapshot_root("empty");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = FsWorkspaceRepository::new(&root);
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(root.clone());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.screen = crate::adapters::tui::app::Screen::Environments;
        let theme = app.theme.clone();

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_envs(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
        let _ = std::fs::remove_dir_all(&root);
    }
}
