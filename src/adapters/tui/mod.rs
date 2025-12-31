mod app;
mod events;
mod theme;
mod ui;
mod widgets;

use crate::use_cases::ScriptService;
use crate::workspace::Workspace;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::error::Error;
use std::io;
use std::time::Duration;

use app::{App, Screen};
use crate::history;
use events::handle_key_event;
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
    terminal.draw(|frame| render_loading(frame))?;
    let entries = service.list_entries(workspace.root())?;
    let history = match history::load_entries(&workspace) {
        Ok(entries) => entries,
        Err(_) => Vec::new(),
    };
    let mut app = App::new(service, workspace, entries, history);

    loop {
        terminal.draw(|frame| render_ui(frame, &mut app))?;

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
            terminal.draw(|frame| render_ui(frame, &mut app))?;
            let run_result = service.run_script(&script, &args);
            let entry = match run_result {
                Ok(output) => history::success_entry(&app.workspace, &script, &args, output),
                Err(err) => history::error_entry(&app.workspace, &script, &args, err.to_string()),
            };
            let _ = history::record_entry(&app.workspace, &entry);
            app.add_history_entry(entry);
            app.back_to_script_select();
            app.reset_run_output_scroll();
            app.screen = Screen::RunResult;
        }
    }
}
