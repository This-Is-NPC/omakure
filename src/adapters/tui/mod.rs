mod app;
mod events;
mod state;
pub(crate) mod theme;
mod ui;
mod widgets;

use crate::search_index::SearchIndex;
use crate::use_cases::ScriptService;
use crate::workspace::Workspace;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::error::Error;
use std::io;
use std::time::Duration;

use crate::runs::{self, RunFilters, RunRow};
use crate::theme_config;
use app::{App, Screen};
use events::handle_key_event;
use theme::load_theme;
use ui::{render_loading, render_ui};

pub fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

pub fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

pub fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    service: &ScriptService,
    workspace: Workspace,
) -> Result<(), Box<dyn Error>> {
    let theme_layout = theme_config::ensure_theme_layout().ok();
    let theme_dir = theme_layout
        .as_ref()
        .map(|layout| layout.themes_dir.as_path());
    let global_theme = theme_layout
        .as_ref()
        .and_then(|layout| theme_config::load_theme_name(&layout.config_path));
    let workspace_theme = theme_config::load_theme_name(workspace.config_path());
    let theme_name = workspace_theme.or(global_theme);
    let theme = load_theme(theme_name.as_deref(), theme_dir);
    terminal.draw(|frame| render_loading(frame, &theme))?;
    let entries = service.list_entries(workspace.scripts_root())?;
    let history = match runs::open(&workspace) {
        Ok(conn) => runs::query_runs(&conn, &RunFilters::default()).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let search_index = SearchIndex::new(workspace.search_db_path());
    // The search index continues to crawl the **global** workspace root —
    // it backs the search screen and is part of the global state contract.
    search_index.start_background_rebuild(workspace.root().to_path_buf());
    let mut app = App::new(service, workspace, entries, history, search_index, theme);

    loop {
        if app.screen == Screen::Search {
            app.refresh_search_status();
        }
        app.poll_widget_load();
        let theme = app.theme.clone();
        terminal.draw(|frame| render_ui(frame, &mut app, &theme))?;

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key_event(&mut app, key)
                }
                _ => {}
            }
        }

        if app.should_quit {
            return Ok(());
        }
        if let Some((script, args)) = app.result.take() {
            app.screen = Screen::Running;
            let theme = app.theme.clone();
            terminal.draw(|frame| render_ui(frame, &mut app, &theme))?;
            let started_at = runs::current_unix_ms();
            let run_result = service.run_script(&script, &args);
            let finished_at = runs::current_unix_ms();
            let row = build_run_row(&script, &args, run_result, started_at, finished_at);
            if let Ok(conn) = runs::open(&app.workspace) {
                let _ = runs::insert_run(&conn, &row);
            }
            app.add_history_entry(row);
            app.back_to_script_select();
            app.reset_run_output_scroll();
            app.screen = Screen::RunResult;
        }
    }
}

fn build_run_row(
    script: &std::path::Path,
    args: &[String],
    run_result: Result<crate::ports::ScriptRunOutput, crate::error::AppError>,
    started_at: i64,
    finished_at: i64,
) -> RunRow {
    let canonical = std::fs::canonicalize(script).unwrap_or_else(|_| script.to_path_buf());
    let script_path = canonical.to_string_lossy().to_string();
    let args_json = serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string());
    let duration_ms = (finished_at - started_at).max(0);
    let omakure_version = crate::app_meta::APP_VERSION.to_string();
    let run_id = runs::generate_run_id();

    match run_result {
        Ok(output) => RunRow {
            run_id,
            script_path,
            script_name: None,
            args_json,
            actor: "human".into(),
            reason: None,
            started_at,
            finished_at,
            duration_ms,
            exit_code: output.exit_code,
            success: output.success,
            stdout: output.stdout,
            stderr: output.stderr,
            error: None,
            parent_run_id: None,
            omakure_version,
        },
        Err(err) => RunRow {
            run_id,
            script_path,
            script_name: None,
            args_json,
            actor: "human".into(),
            reason: None,
            started_at,
            finished_at,
            duration_ms,
            exit_code: None,
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            error: Some(err.to_string()),
            parent_run_id: None,
            omakure_version,
        },
    }
}
