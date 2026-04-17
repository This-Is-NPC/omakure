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

use crate::run_executor::{execute_with_heartbeat, ExecutionTerminal};
use crate::runs::{self, EnqueueOptions, RunFilters, RunRow, RunStateSet};
use crate::theme_config;
use app::{App, Screen};
use events::handle_key_event;
use std::sync::mpsc::{self, TryRecvError};
use theme::load_theme;
use ui::{render_loading, render_ui};

/// How often the background poller re-reads the runs DB. Small enough
/// to feel live (scheduled runs show up within ~1s), large enough that
/// we never hammer SQLite even if the TUI is left idle for hours.
const HISTORY_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

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
    let history = load_history(&workspace);
    let search_index = SearchIndex::new(workspace.search_db_path());
    // The search index continues to crawl the **global** workspace root —
    // it backs the search screen and is part of the global state contract.
    search_index.start_background_rebuild(workspace.root().to_path_buf());
    let mut app = App::new(service, workspace, entries, history, search_index, theme);
    let history_rx = spawn_history_poller(app.workspace.clone_for_executor());

    loop {
        if app.screen == Screen::Search {
            app.refresh_search_status();
        }
        app.poll_widget_load();
        poll_inline_run(&mut app);
        poll_history_refresh(&mut app, &history_rx);
        let theme = app.theme.clone();
        terminal.draw(|frame| render_ui(frame, &mut app, &theme))?;
        app.tick = app.tick.wrapping_add(1);

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
            start_inline_run(&mut app, script, args);
        }
    }
}

/// Spawn a worker thread that drives an inline script execution
/// through `run_through_state_machine` and sends the resulting row
/// back over an mpsc channel. The main loop keeps drawing and
/// incrementing `app.tick` while the worker runs, so the Sand spinner
/// on the Running screen animates instead of freezing on a single
/// frame. Mirrors the existing `App::start_widget_load` pattern.
fn start_inline_run(app: &mut App, script: std::path::PathBuf, args: Vec<String>) {
    let workspace = app.workspace.clone_for_executor();
    let (tx, rx) = mpsc::channel();
    app.inline_run_receiver = Some(rx);
    app.screen = Screen::Running;
    std::thread::spawn(move || {
        let row = run_through_state_machine(&workspace, &script, &args);
        let _ = tx.send(row);
    });
}

/// Drain a completed inline run from the worker thread, if any. On
/// success, append the row to history and transition to `RunResult`.
/// On `Disconnected` (worker panicked or channel dropped), bail out
/// to `RunResult` so the user is not stranded on the Running screen.
fn poll_inline_run(app: &mut App) {
    let Some(receiver) = &app.inline_run_receiver else {
        return;
    };
    match receiver.try_recv() {
        Ok(row) => {
            if let Some(row) = row {
                app.add_history_entry(row);
            }
            app.back_to_script_select();
            app.reset_run_output_scroll();
            app.screen = Screen::RunResult;
            app.inline_run_receiver = None;
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            app.back_to_script_select();
            app.reset_run_output_scroll();
            app.screen = Screen::RunResult;
            app.inline_run_receiver = None;
        }
    }
}

/// Insert a `running` row, drive the script through the shared executor,
/// transition to the correct terminal state, and return the final row so
/// the TUI history view can display it. The TUI does not use a worker
/// daemon — it always uses the inline fast path, just like `omakure run`.
fn run_through_state_machine(
    workspace: &Workspace,
    script: &std::path::Path,
    args: &[String],
) -> Option<RunRow> {
    let canonical = std::fs::canonicalize(script).unwrap_or_else(|_| script.to_path_buf());
    let canonical_str = canonical.to_string_lossy().to_string();
    let conn = runs::open(workspace).ok()?;
    let row = runs::start_inline(
        &conn,
        &canonical_str,
        args,
        &format!("inline:{}", std::process::id()),
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: crate::app_meta::APP_VERSION.to_string(),
            ..Default::default()
        },
    )
    .ok()?;
    drop(conn);

    let result = execute_with_heartbeat(workspace, &row, vec![], None);
    let conn = runs::open(workspace).ok()?;
    let _ = match result.terminal {
        ExecutionTerminal::Completed => runs::complete(&conn, &row.run_id, result.completion),
        ExecutionTerminal::Failed | ExecutionTerminal::Errored => {
            runs::fail(&conn, &row.run_id, result.completion)
        }
        ExecutionTerminal::TimedOut => runs::time_out(&conn, &row.run_id, result.completion),
        ExecutionTerminal::Cancelled => {
            runs::record_cancelled_output(&conn, &row.run_id, result.completion)
        }
    };
    runs::get_run(&conn, &row.run_id).ok().flatten()
}

/// Spawn a background thread that re-reads the runs DB once per
/// `HISTORY_REFRESH_INTERVAL` and streams fresh `Vec<RunRow>` snapshots
/// back to the main loop over an mpsc channel. Keeps the TUI reactive
/// to scheduled runs without blocking `terminal.draw` or `event::poll`.
///
/// When the receiver is dropped at shutdown, `send` fails and the thread
/// exits on its own — no join handle needed.
fn spawn_history_poller(workspace: Workspace) -> mpsc::Receiver<Vec<RunRow>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let entries = load_history(&workspace);
        if tx.send(entries).is_err() {
            break;
        }
        std::thread::sleep(HISTORY_REFRESH_INTERVAL);
    });
    rx
}

/// Drain any pending history snapshots from the background poller. We
/// only apply the most recent one — intermediate snapshots are stale by
/// the time we see them, and applying them would waste work.
fn poll_history_refresh(app: &mut App, rx: &mpsc::Receiver<Vec<RunRow>>) {
    let mut latest: Option<Vec<RunRow>> = None;
    while let Ok(entries) = rx.try_recv() {
        latest = Some(entries);
    }
    if let Some(entries) = latest {
        app.apply_history_refresh(entries);
    }
}

fn load_history(workspace: &Workspace) -> Vec<RunRow> {
    match runs::open(workspace) {
        Ok(conn) => runs::query_runs(
            &conn,
            &RunFilters {
                states: RunStateSet::All.to_states(),
                ..Default::default()
            },
        )
        .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}
