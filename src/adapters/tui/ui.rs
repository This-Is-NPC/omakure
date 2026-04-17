use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::app::{App, Screen};
use super::theme::Theme;
use super::widgets::{
    dashboards, environment, envs, error as error_widget, field_input, history,
    loading as loading_widget, run_result, running, schedules, schema, scripts, search,
};

pub(crate) fn render_ui(frame: &mut Frame, app: &mut App, theme: &Theme) {
    match app.screen {
        Screen::ScriptSelect => render_script_select(frame, app, theme),
        Screen::Search => search::render_search(frame, frame.size(), app, theme),
        Screen::Environments => envs::render_envs(frame, frame.size(), app, theme),
        Screen::FieldInput => field_input::render_field_input(frame, frame.size(), app, theme),
        Screen::History => history::render_history(frame, frame.size(), app, theme),
        Screen::Running => running::render_running(frame, frame.size(), app, theme),
        Screen::RunResult => run_result::render_run_result(frame, frame.size(), app, theme),
        Screen::Error => render_error(frame, app, theme),
        Screen::Schedules => schedules::render_schedules(frame, frame.size(), app, theme),
    }
}

pub(crate) fn render_loading(frame: &mut Frame, theme: &Theme) {
    // The bootstrap loading screen runs before any `App` exists, so it
    // does not have a real frame counter. Tick = 0 still renders the
    // first frame of the spinner — animation kicks in once the main
    // loop takes over and starts incrementing `App.tick`.
    loading_widget::render_loading(frame, frame.size(), theme, 0);
}

fn render_script_select(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let (info_title, info_lines) = environment::status_info(
        &app.workspace,
        app.navigation.widget.as_ref(),
        app.navigation.widget_error.as_deref(),
        app.navigation.widget_loading,
        theme,
        app.tick,
    );
    let info_height = info_lines.len() as u16 + 2;

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(omakure_title_line(theme));
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

    let selected_script_path = match app.selected_entry() {
        Some(entry) if entry.kind == crate::ports::WorkspaceEntryKind::Script => {
            Some(entry.path.clone())
        }
        _ => None,
    };

    if app.script_dashboard_expanded {
        if let Some(script_path) = selected_script_path.as_ref() {
            // Fullscreen per-script charts. The list is hidden until
            // the user presses Esc to collapse the expanded view.
            dashboards::render_script_charts(frame, entries_area, app, theme, script_path, true);
        } else {
            // Fallback: nothing meaningful to expand. Render the
            // normal list (defensive — `e` is gated on script
            // selection so this branch is unreachable in practice).
            scripts::render_scripts(
                frame,
                entries_area,
                &app.workspace,
                &app.navigation.current_dir,
                &app.navigation.entries,
                &mut app.navigation.list_state,
                theme,
            );
        }
    } else if let Some(script_path) = selected_script_path.as_ref() {
        // Compact layout: scripts list on the left, vertical stack of
        // Schema (top) + Charts (bottom) on the right.
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(entries_area);

        scripts::render_scripts(
            frame,
            body_chunks[0],
            &app.workspace,
            &app.navigation.current_dir,
            &app.navigation.entries,
            &mut app.navigation.list_state,
            theme,
        );

        // Dynamic right pane:
        //   • Schema grows with its content up to half the pane height.
        //   • Any remaining space goes to the per-script charts.
        // When the schema overflows its cap it renders with an internal
        // scroll offset (see `schema_preview_scroll` on NavigationState,
        // driven by events.rs).
        let right_h = body_chunks[1].height;
        let schema_content_h = schema::schema_preview_height(
            app.navigation.schema_preview.as_ref(),
            app.navigation.schema_preview_error.as_deref(),
            theme,
        )
        .saturating_add(2); // +2 for block borders
        let schema_cap = right_h / 2;
        let schema_h = schema_content_h.min(schema_cap).max(3);
        let rest_h = right_h.saturating_sub(schema_h);

        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(schema_h), Constraint::Length(rest_h)])
            .split(body_chunks[1]);

        let schema_title = schema_title(app);
        let schema_scroll = if schema_content_h > schema_cap {
            app.navigation.schema_preview_scroll
        } else {
            0
        };
        schema::render_schema_preview(
            frame,
            right_chunks[0],
            &schema_title,
            app.navigation.schema_preview.as_ref(),
            app.navigation.schema_preview_error.as_deref(),
            theme,
            schema_scroll,
        );
        dashboards::render_script_charts(frame, right_chunks[1], app, theme, script_path, false);
    } else {
        scripts::render_scripts(
            frame,
            entries_area,
            &app.workspace,
            &app.navigation.current_dir,
            &app.navigation.entries,
            &mut app.navigation.list_state,
            theme,
        );
    }

    let footer_text = build_script_select_footer(app);
    let env_label = build_env_label(app);
    let env_label_width = env_label.chars().count() as u16;

    if env_label_width == 0 {
        let footer = Paragraph::new(footer_text).style(theme.text_secondary());
        frame.render_widget(footer, chunks[2]);
    } else {
        let footer_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(env_label_width)])
            .split(chunks[2]);
        let footer = Paragraph::new(footer_text).style(theme.text_secondary());
        frame.render_widget(footer, footer_chunks[0]);
        let env = Paragraph::new(env_label)
            .style(theme.text_secondary())
            .alignment(ratatui::layout::Alignment::Right);
        frame.render_widget(env, footer_chunks[1]);
    }
}

fn build_env_label(app: &App) -> String {
    let active = app
        .environment
        .config
        .as_ref()
        .and_then(|c| c.active.clone());
    match active {
        Some(name) => format!("env: {}", name),
        None => "env: —".to_string(),
    }
}

const PREFIX_HINT: &str = "Ctrl+/ [s]earch [e]nvs [h]istory [c]schedules [q]uit";
const PREFIX_PENDING_LABEL: &str = "-- PREFIX --";

fn build_script_select_footer(app: &App) -> String {
    if app.prefix_pending {
        return PREFIX_PENDING_LABEL.to_string();
    }
    if app.script_dashboard_expanded {
        return format!("Esc collapse, e collapse, Enter run, {PREFIX_HINT}");
    }
    let has_script = matches!(
        app.selected_entry(),
        Some(entry) if entry.kind == crate::ports::WorkspaceEntryKind::Script
    );
    let expand_hint = if has_script { ", e expand charts" } else { "" };
    let nav_hint = if app.navigation.current_dir != app.workspace.root() {
        ", Backspace up"
    } else {
        ""
    };
    if app.navigation.entries.is_empty() {
        format!("Folder is empty{nav_hint}, r refresh, {PREFIX_HINT}")
    } else {
        format!("Up/Down move, Enter open/run{nav_hint}{expand_hint}, r refresh, {PREFIX_HINT}")
    }
}

fn render_error(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let message = app
        .error_message
        .as_deref()
        .unwrap_or("Unknown error while loading schema");
    error_widget::render_error(frame, frame.size(), message, theme);
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

fn omakure_title_line(theme: &Theme) -> Line<'static> {
    gradient_line(
        "omakure",
        theme.brand.gradient_start.color(),
        theme.brand.gradient_end.color(),
    )
}

fn gradient_line(text: &str, start: Color, end: Color) -> Line<'static> {
    let start = color_to_tuple(start);
    let end = color_to_tuple(end);
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
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn lerp_color(start: (u8, u8, u8), end: (u8, u8, u8), t: f32) -> Color {
    let lerp = |a, b| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::Rgb(
        lerp(start.0, end.0),
        lerp(start.1, end.1),
        lerp(start.2, end.2),
    )
}

fn color_to_tuple(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (255, 0, 0),
        Color::Green => (0, 255, 0),
        Color::Yellow => (255, 255, 0),
        Color::Blue => (0, 0, 255),
        Color::Magenta => (255, 0, 255),
        Color::Cyan => (0, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (64, 64, 64),
        Color::LightRed => (255, 128, 128),
        Color::LightGreen => (128, 255, 128),
        Color::LightYellow => (255, 255, 128),
        Color::LightBlue => (128, 128, 255),
        Color::LightMagenta => (255, 128, 255),
        Color::LightCyan => (128, 255, 255),
        Color::White => (255, 255, 255),
        Color::Indexed(value) => (value, value, value),
        Color::Reset => (255, 255, 255),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::ports::{WorkspaceEntry, WorkspaceEntryKind};
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tempfile::TempDir;

    fn make_app<'a>(tmp: &'a TempDir, svc: &'a ScriptService) -> App<'a> {
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let entries = vec![WorkspaceEntry {
            path: tmp.path().join("deploy.sh"),
            kind: WorkspaceEntryKind::Script,
        }];
        App::test_new(svc, ws, entries, vec![])
    }

    fn make_svc(tmp: &TempDir) -> ScriptService {
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        ScriptService::new(Box::new(repo), Box::new(runner))
    }

    #[test]
    fn render_ui_script_select_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_search_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.screen = Screen::Search;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_environments_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.screen = Screen::Environments;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_field_input_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_history_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.screen = Screen::History;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_running_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.screen = Screen::Running;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_run_result_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.screen = Screen::RunResult;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_error_no_panic() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.screen = Screen::Error;
        app.error_message = Some("test error".to_string());
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_loading_no_panic() {
        let theme = Theme::default();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_loading(f, &theme)).unwrap();
    }

    #[test]
    fn test_gradient_line_length() {
        let line = gradient_line("hello", Color::Red, Color::Blue);
        assert_eq!(line.spans.len(), 5);
    }

    #[test]
    fn test_lerp_color_midpoint() {
        let result = lerp_color((0, 0, 0), (100, 200, 50), 0.5);
        assert_eq!(result, Color::Rgb(50, 100, 25));
    }

    #[test]
    fn test_color_to_tuple_rgb() {
        assert_eq!(color_to_tuple(Color::Rgb(10, 20, 30)), (10, 20, 30));
    }

    #[test]
    fn test_color_to_tuple_named() {
        assert_eq!(color_to_tuple(Color::Red), (255, 0, 0));
        assert_eq!(color_to_tuple(Color::Black), (0, 0, 0));
        assert_eq!(color_to_tuple(Color::White), (255, 255, 255));
    }

    #[test]
    fn test_build_script_select_footer_empty() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let app = App::test_new(&svc, ws, vec![], vec![]);
        let footer = build_script_select_footer(&app);
        assert!(footer.contains("empty"));
        assert!(footer.contains("refresh"));
    }

    #[test]
    fn test_build_script_select_footer_with_entries() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let app = make_app(&tmp, &svc);
        let footer = build_script_select_footer(&app);
        assert!(footer.contains("Up/Down"));
        assert!(footer.contains("Enter"));
    }

    #[test]
    fn test_build_script_select_footer_dashboard_expanded() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.script_dashboard_expanded = true;
        let footer = build_script_select_footer(&app);
        assert!(footer.contains("collapse"));
    }

    #[test]
    fn test_color_to_tuple_full_palette() {
        assert_eq!(color_to_tuple(Color::Green), (0, 255, 0));
        assert_eq!(color_to_tuple(Color::Yellow), (255, 255, 0));
        assert_eq!(color_to_tuple(Color::Blue), (0, 0, 255));
        assert_eq!(color_to_tuple(Color::Magenta), (255, 0, 255));
        assert_eq!(color_to_tuple(Color::Cyan), (0, 255, 255));
        assert_eq!(color_to_tuple(Color::Gray), (128, 128, 128));
        assert_eq!(color_to_tuple(Color::DarkGray), (64, 64, 64));
        assert_eq!(color_to_tuple(Color::LightRed), (255, 128, 128));
        assert_eq!(color_to_tuple(Color::LightGreen), (128, 255, 128));
        assert_eq!(color_to_tuple(Color::LightYellow), (255, 255, 128));
        assert_eq!(color_to_tuple(Color::LightBlue), (128, 128, 255));
        assert_eq!(color_to_tuple(Color::LightMagenta), (255, 128, 255));
        assert_eq!(color_to_tuple(Color::LightCyan), (128, 255, 255));
        assert_eq!(color_to_tuple(Color::Indexed(7)), (7, 7, 7));
        assert_eq!(color_to_tuple(Color::Reset), (255, 255, 255));
    }

    #[test]
    fn test_gradient_line_single_char_uses_zero_t() {
        let line = gradient_line("x", Color::Red, Color::Blue);
        assert_eq!(line.spans.len(), 1);
    }

    #[test]
    fn test_schema_title_for_none_or_directory() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        assert_eq!(schema_title(&app), "Schema");

        let dir_entry = WorkspaceEntry {
            path: tmp.path().join("subdir"),
            kind: WorkspaceEntryKind::Directory,
        };
        app.navigation.entries = vec![dir_entry];
        app.navigation.selection = 0;
        assert_eq!(schema_title(&app), "Schema");
    }

    #[test]
    fn test_schema_title_with_script_includes_filename() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let app = make_app(&tmp, &svc);
        let title = schema_title(&app);
        assert!(title.starts_with("Schema:"));
        assert!(title.contains("deploy.sh"));
    }

    #[test]
    fn render_ui_script_select_with_dashboard_expanded() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.script_dashboard_expanded = true;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn render_ui_script_select_with_no_selected_script() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let dir_entry = WorkspaceEntry {
            path: tmp.path().join("subdir"),
            kind: WorkspaceEntryKind::Directory,
        };
        let mut app = App::test_new(&svc, ws, vec![dir_entry], vec![]);
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_ui(f, &mut app, &theme)).unwrap();
    }

    #[test]
    fn env_label_shows_placeholder_when_no_active_env() {
        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let app = make_app(&tmp, &svc);
        let label = build_env_label(&app);
        assert_eq!(label, "env: —");
    }

    #[test]
    fn env_label_shows_active_env_name() {
        use crate::ports::EnvironmentConfig;
        use std::collections::HashMap;

        let tmp = TempDir::new().unwrap();
        let svc = make_svc(&tmp);
        let mut app = make_app(&tmp, &svc);
        app.environment.config = Some(EnvironmentConfig {
            envs_dir: tmp.path().join("envs"),
            active: Some("dev.conf".to_string()),
            defaults: HashMap::new(),
            session_conf_path: None,
        });
        let label = build_env_label(&app);
        assert_eq!(label, "env: dev.conf");
    }
}
