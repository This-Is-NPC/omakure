use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, HistoryFocus, Screen};

pub(crate) fn handle_key_event(app: &mut App, key: KeyEvent) {
    match app.screen {
        Screen::ScriptSelect => handle_list_key(app, key),
        Screen::Search => handle_search_key(app, key),
        Screen::FieldInput => handle_input_key(app, key),
        Screen::History => handle_history_key(app, key),
        Screen::Running => {}
        Screen::RunResult => handle_run_result_key(app, key),
        Screen::Error => handle_error_key(app, key),
    }
}

fn handle_list_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('s') | KeyCode::Char('S')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.enter_search()
        }
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc => {
            if app.current_dir == app.workspace.root() {
                app.should_quit = true;
            } else {
                app.navigate_up();
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => app.refresh_entries(),
        KeyCode::Char('i') | KeyCode::Char('I') | KeyCode::F(6) => app.refresh_status(),
        KeyCode::Char('h') | KeyCode::Char('H') => {
            app.screen = Screen::History;
            app.history_focus = HistoryFocus::List;
            app.reset_run_output_scroll();
        }
        KeyCode::Backspace | KeyCode::Left => app.navigate_up(),
        _ if app.entries.is_empty() => {}
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::Enter => app.enter_selected(),
        _ => {}
    }
}

fn handle_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.screen = Screen::ScriptSelect,
        KeyCode::Down | KeyCode::Char('j') => app.move_search_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_search_selection(-1),
        KeyCode::Enter => app.open_selected_search(),
        KeyCode::Backspace => app.pop_search_char(),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.append_search_char(c)
        }
        _ => {}
    }
}

fn handle_input_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.back_to_script_select(),
        KeyCode::Char('b') | KeyCode::Char('B') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.back_to_script_select()
        }
        KeyCode::Enter => app.submit_form(),
        KeyCode::Tab => app.move_field_selection(1),
        KeyCode::BackTab => app.move_field_selection(-1),
        KeyCode::Down => app.move_field_selection(1),
        KeyCode::Up => app.move_field_selection(-1),
        KeyCode::Backspace => app.pop_field_char(),
        KeyCode::Char(c) => app.append_field_char(c),
        _ => {}
    }
}

fn handle_error_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Enter => {
            app.error = None;
            app.screen = Screen::ScriptSelect;
        }
        _ => {}
    }
}

fn handle_history_key(app: &mut App, key: KeyEvent) {
    match app.history_focus {
        HistoryFocus::List => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => app.screen = Screen::ScriptSelect,
            KeyCode::Down | KeyCode::Char('j') => app.move_history_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_history_selection(-1),
            KeyCode::Enter | KeyCode::Right => {
                app.history_focus = HistoryFocus::Output;
                app.reset_run_output_scroll();
            }
            _ => {}
        },
        HistoryFocus::Output => match key.code {
            KeyCode::Char('q') => app.screen = Screen::ScriptSelect,
            KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                app.history_focus = HistoryFocus::List
            }
            KeyCode::Down | KeyCode::Char('j') => app.scroll_run_output(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_run_output(-1),
            KeyCode::PageDown => app.scroll_run_output(10),
            KeyCode::PageUp => app.scroll_run_output(-10),
            KeyCode::Home => app.run_output_scroll = 0,
            KeyCode::End => app.run_output_scroll = u16::MAX,
            _ => {}
        },
    }
}

fn handle_run_result_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
            app.screen = Screen::ScriptSelect
        }
        KeyCode::Char('h') | KeyCode::Char('H') => {
            app.screen = Screen::History;
            app.history_focus = HistoryFocus::List;
            app.reset_run_output_scroll();
        }
        KeyCode::Down | KeyCode::Char('j') => app.scroll_run_output(1),
        KeyCode::Up | KeyCode::Char('k') => app.scroll_run_output(-1),
        KeyCode::PageDown => app.scroll_run_output(10),
        KeyCode::PageUp => app.scroll_run_output(-10),
        KeyCode::Home => app.run_output_scroll = 0,
        _ => {}
    }
}
