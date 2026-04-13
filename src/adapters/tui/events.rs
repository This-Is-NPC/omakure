use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, HistoryFocus, HistoryView, Screen};

pub(crate) fn handle_key_event(app: &mut App, key: KeyEvent) {
    match app.screen {
        Screen::ScriptSelect => handle_list_key(app, key),
        Screen::Search => handle_search_key(app, key),
        Screen::Environments => handle_envs_key(app, key),
        Screen::FieldInput => handle_input_key(app, key),
        Screen::History => handle_history_key(app, key),
        Screen::Running => {}
        Screen::RunResult => handle_run_result_key(app, key),
        Screen::Error => handle_error_key(app, key),
    }
}

fn handle_list_key(app: &mut App, key: KeyEvent) {
    // `Alt+E` (envs) is checked before any non-modified `e` shortcut so
    // the modifier branch always wins.
    if matches!(key.code, KeyCode::Char('e') | KeyCode::Char('E'))
        && key.modifiers.contains(KeyModifiers::ALT)
    {
        app.enter_envs();
        return;
    }
    match key.code {
        KeyCode::Char('s') | KeyCode::Char('S')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.enter_search()
        }
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc => {
            // Esc collapses the per-script dashboard expansion first.
            if app.script_dashboard_expanded {
                app.script_dashboard_expanded = false;
            } else if app.navigation.current_dir == app.workspace.root() {
                app.should_quit = true;
            } else {
                app.navigate_up();
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => app.refresh_entries(),
        KeyCode::Char('i') | KeyCode::Char('I') | KeyCode::F(6) => app.refresh_status(),
        KeyCode::Char('h') | KeyCode::Char('H') => {
            app.screen = Screen::History;
            app.history.focus = HistoryFocus::List;
            app.reset_run_output_scroll();
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            // `e` toggles the per-script dashboard expansion, but only
            // when a script (not a directory) is highlighted. Pressing
            // `e` over a directory is a no-op.
            if matches!(
                app.selected_entry(),
                Some(entry) if entry.kind == crate::ports::WorkspaceEntryKind::Script
            ) {
                app.script_dashboard_expanded = !app.script_dashboard_expanded;
            }
        }
        KeyCode::Backspace | KeyCode::Left => app.navigate_up(),
        _ if app.navigation.entries.is_empty() => {}
        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::Enter => app.enter_selected(),
        _ => {}
    }
}

fn handle_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.screen = Screen::ScriptSelect,
        KeyCode::Char('e') | KeyCode::Char('E') if key.modifiers.contains(KeyModifiers::ALT) => {
            app.enter_envs()
        }
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
        KeyCode::Char('b') | KeyCode::Char('B')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
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
            app.error_message = None;
            app.screen = Screen::ScriptSelect;
        }
        _ => {}
    }
}

fn handle_history_key(app: &mut App, key: KeyEvent) {
    // `Alt+E` always routes to the envs screen, regardless of view, so
    // it must be checked before any non-modified `e` shortcut below.
    if matches!(key.code, KeyCode::Char('e') | KeyCode::Char('E'))
        && key.modifiers.contains(KeyModifiers::ALT)
    {
        app.enter_envs();
        return;
    }

    // `Tab` toggles List <-> Dashboards in either view, regardless of
    // the inner `HistoryFocus` of the List view.
    if matches!(key.code, KeyCode::Tab) {
        app.history.toggle_view();
        return;
    }

    match app.history.view {
        HistoryView::List => handle_history_list_key(app, key),
        HistoryView::Dashboards => handle_history_dashboards_key(app, key),
    }
}

fn handle_history_list_key(app: &mut App, key: KeyEvent) {
    match app.history.focus {
        HistoryFocus::List => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => app.screen = Screen::ScriptSelect,
            KeyCode::Down | KeyCode::Char('j') => app.move_history_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_history_selection(-1),
            KeyCode::Enter | KeyCode::Right => {
                app.history.focus = HistoryFocus::Output;
                app.reset_run_output_scroll();
            }
            _ => {}
        },
        HistoryFocus::Output => match key.code {
            KeyCode::Char('q') => app.screen = Screen::ScriptSelect,
            KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                app.history.focus = HistoryFocus::List
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

fn handle_history_dashboards_key(app: &mut App, key: KeyEvent) {
    let has_selection = !app.history.entries.is_empty();
    match key.code {
        KeyCode::Char('q') => app.screen = Screen::ScriptSelect,
        KeyCode::Esc => {
            // Esc collapses an expanded per-script panel first; only
            // when already in the split layout does it leave History.
            if app.history.dashboards_escape() {
                app.screen = Screen::ScriptSelect;
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') | KeyCode::Enter if has_selection => {
            app.history.toggle_dashboard_expand();
        }
        KeyCode::Down | KeyCode::Char('j') => app.move_history_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_history_selection(-1),
        _ => {}
    }
}

fn handle_run_result_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => app.screen = Screen::ScriptSelect,
        KeyCode::Char('h') | KeyCode::Char('H') => {
            app.screen = Screen::History;
            app.history.focus = HistoryFocus::List;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::ports::{WorkspaceEntry, WorkspaceEntryKind};
    use crate::use_cases::ScriptService;
    use crate::workspace::Workspace;
    use tempfile::TempDir;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn setup_app<'a>(tmp: &'a TempDir, service: &'a ScriptService) -> App<'a> {
        let ws = Workspace::new(tmp.path().to_path_buf());
        let entries = vec![
            WorkspaceEntry {
                path: tmp.path().join("infra"),
                kind: WorkspaceEntryKind::Directory,
            },
            WorkspaceEntry {
                path: tmp.path().join("deploy.sh"),
                kind: WorkspaceEntryKind::Script,
            },
        ];
        App::test_new(service, ws, entries, vec![])
    }

    fn make_service(tmp: &TempDir) -> ScriptService {
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        ScriptService::new(Box::new(repo), Box::new(runner))
    }

    #[test]
    fn test_q_quits_from_script_select() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn test_j_moves_selection_down() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        assert_eq!(app.navigation.selection, 0);
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.navigation.selection, 1);
    }

    #[test]
    fn test_k_moves_selection_up() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.move_selection(1); // go to index 1
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.navigation.selection, 0);
    }

    #[test]
    fn test_h_enters_history() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Char('h')));
        assert_eq!(app.screen, Screen::History);
    }

    #[test]
    fn test_ctrl_s_enters_search() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key_mod(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(app.screen, Screen::Search);
    }

    #[test]
    fn test_esc_from_search_returns_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_search_typing_appends_chars() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        handle_key_event(&mut app, key(KeyCode::Char('d')));
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert_eq!(app.search.query, "de");
    }

    #[test]
    fn test_search_backspace_pops_char() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        app.search.query = "abc".to_string();
        handle_key_event(&mut app, key(KeyCode::Backspace));
        assert_eq!(app.search.query, "ab");
    }

    #[test]
    fn test_error_enter_returns_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Error;
        app.error_message = Some("oops".to_string());
        handle_key_event(&mut app, key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ScriptSelect);
        assert!(app.error_message.is_none());
    }

    #[test]
    fn test_error_q_quits() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Error;
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn test_history_tab_toggles_view() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        assert_eq!(app.history.view, HistoryView::List);
        handle_key_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.history.view, HistoryView::Dashboards);
        handle_key_event(&mut app, key(KeyCode::Tab));
        assert_eq!(app.history.view, HistoryView::List);
    }

    #[test]
    fn test_history_q_returns_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_history_list_enter_focuses_output() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.focus = HistoryFocus::List;
        handle_key_event(&mut app, key(KeyCode::Enter));
        assert_eq!(app.history.focus, HistoryFocus::Output);
    }

    #[test]
    fn test_history_output_esc_returns_to_list() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.focus = HistoryFocus::Output;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.history.focus, HistoryFocus::List);
    }

    #[test]
    fn test_run_result_esc_returns() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::RunResult;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_run_result_h_goes_to_history() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::RunResult;
        handle_key_event(&mut app, key(KeyCode::Char('h')));
        assert_eq!(app.screen, Screen::History);
    }

    #[test]
    fn test_field_input_esc_returns() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_field_input_char_appends() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        app.field_input.fields = vec![crate::domain::Field {
            name: "target".to_string(),
            prompt: None,
            kind: "string".to_string(),
            order: 0,
            required: None,
            default: None,
            choices: None,
            arg: None,
        }];
        app.field_input.field_inputs = vec![String::new()];
        handle_key_event(&mut app, key(KeyCode::Char('a')));
        assert_eq!(app.field_input.field_inputs[0], "a");
    }

    #[test]
    fn test_running_key_is_noop() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Running;
        let screen_before = app.screen;
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.screen, screen_before);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_alt_e_enters_envs_from_script_select() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key_mod(KeyCode::Char('e'), KeyModifiers::ALT));
        assert_eq!(app.screen, Screen::Environments);
    }

    #[test]
    fn test_esc_quits_at_workspace_root() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn test_esc_collapses_dashboard_expansion_first() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.script_dashboard_expanded = true;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert!(!app.script_dashboard_expanded);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_e_toggles_script_dashboard_only_for_script() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        // Move to entry index 1 which is a Script in setup_app.
        app.move_selection(1);
        assert!(!app.script_dashboard_expanded);
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert!(app.script_dashboard_expanded);
        // Toggling again collapses.
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert!(!app.script_dashboard_expanded);
    }

    #[test]
    fn test_e_over_directory_is_noop() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        // Index 0 is a Directory in setup_app.
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert!(!app.script_dashboard_expanded);
    }

    #[test]
    fn test_r_refreshes_entries() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        // Just exercise the path.
        handle_key_event(&mut app, key(KeyCode::Char('r')));
        handle_key_event(&mut app, key(KeyCode::F(5)));
        handle_key_event(&mut app, key(KeyCode::Char('i')));
        handle_key_event(&mut app, key(KeyCode::F(6)));
    }

    #[test]
    fn test_backspace_navigates_up() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Backspace));
        // No assertion on path — just exercise navigate_up.
    }

    #[test]
    fn test_search_alt_e_enters_envs() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        handle_key_event(&mut app, key_mod(KeyCode::Char('e'), KeyModifiers::ALT));
        assert_eq!(app.screen, Screen::Environments);
    }

    #[test]
    fn test_search_navigation_keys() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        handle_key_event(&mut app, key(KeyCode::Down));
        handle_key_event(&mut app, key(KeyCode::Up));
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        handle_key_event(&mut app, key(KeyCode::Enter));
    }

    #[test]
    fn test_field_input_navigation_keys() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        app.field_input.fields = vec![
            crate::domain::Field {
                name: "a".to_string(),
                prompt: None,
                kind: "string".to_string(),
                order: 0,
                required: None,
                default: None,
                choices: None,
                arg: None,
            },
            crate::domain::Field {
                name: "b".to_string(),
                prompt: None,
                kind: "string".to_string(),
                order: 1,
                required: None,
                default: None,
                choices: None,
                arg: None,
            },
        ];
        app.field_input.field_inputs = vec![String::new(), String::new()];

        handle_key_event(&mut app, key(KeyCode::Tab));
        handle_key_event(&mut app, key(KeyCode::BackTab));
        handle_key_event(&mut app, key(KeyCode::Down));
        handle_key_event(&mut app, key(KeyCode::Up));
        handle_key_event(&mut app, key(KeyCode::Char('x')));
        handle_key_event(&mut app, key(KeyCode::Backspace));
        handle_key_event(&mut app, key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_history_alt_e_enters_envs() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        handle_key_event(&mut app, key_mod(KeyCode::Char('e'), KeyModifiers::ALT));
        assert_eq!(app.screen, Screen::Environments);
    }

    #[test]
    fn test_history_list_navigation_and_right() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.focus = HistoryFocus::List;
        handle_key_event(&mut app, key(KeyCode::Down));
        handle_key_event(&mut app, key(KeyCode::Up));
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        handle_key_event(&mut app, key(KeyCode::Right));
        assert_eq!(app.history.focus, HistoryFocus::Output);
    }

    #[test]
    fn test_history_output_scroll_and_paging_keys() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.focus = HistoryFocus::Output;
        handle_key_event(&mut app, key(KeyCode::Down));
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        handle_key_event(&mut app, key(KeyCode::PageDown));
        handle_key_event(&mut app, key(KeyCode::PageUp));
        handle_key_event(&mut app, key(KeyCode::Up));
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        handle_key_event(&mut app, key(KeyCode::Home));
        assert_eq!(app.run_output_scroll, 0);
        handle_key_event(&mut app, key(KeyCode::End));
        assert_eq!(app.run_output_scroll, u16::MAX);
        handle_key_event(&mut app, key(KeyCode::Left));
        assert_eq!(app.history.focus, HistoryFocus::List);
    }

    #[test]
    fn test_history_output_q_quits_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.focus = HistoryFocus::Output;
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_history_dashboards_navigation_and_quit() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.view = HistoryView::Dashboards;
        handle_key_event(&mut app, key(KeyCode::Down));
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        handle_key_event(&mut app, key(KeyCode::Up));
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        handle_key_event(&mut app, key(KeyCode::Esc));
        // Esc returns to ScriptSelect (no expansion to collapse).
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_history_dashboards_q_returns_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.view = HistoryView::Dashboards;
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_run_result_q_and_enter_return_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::RunResult;
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.screen, Screen::ScriptSelect);

        app.screen = Screen::RunResult;
        handle_key_event(&mut app, key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn test_run_result_scroll_keys() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::RunResult;
        handle_key_event(&mut app, key(KeyCode::Down));
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        handle_key_event(&mut app, key(KeyCode::Up));
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        handle_key_event(&mut app, key(KeyCode::PageDown));
        handle_key_event(&mut app, key(KeyCode::PageUp));
        handle_key_event(&mut app, key(KeyCode::Home));
        assert_eq!(app.run_output_scroll, 0);
    }

    #[test]
    fn test_envs_screen_keys_dispatch() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Environments;
        handle_key_event(&mut app, key(KeyCode::Down));
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        handle_key_event(&mut app, key(KeyCode::Up));
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        handle_key_event(&mut app, key(KeyCode::PageDown));
        handle_key_event(&mut app, key(KeyCode::PageUp));
        handle_key_event(&mut app, key(KeyCode::Home));
        handle_key_event(&mut app, key(KeyCode::End));
        handle_key_event(&mut app, key(KeyCode::Char('r')));
        handle_key_event(&mut app, key(KeyCode::Char('d')));
        handle_key_event(&mut app, key(KeyCode::Enter));
        // Esc exits.
        handle_key_event(&mut app, key(KeyCode::Esc));
    }

    #[test]
    fn test_field_input_enter_submits_form() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        // Empty form: submit_form just walks an empty path.
        handle_key_event(&mut app, key(KeyCode::Enter));
    }
}

fn handle_envs_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.exit_envs(),
        KeyCode::Char('r') | KeyCode::Char('R') => app.refresh_status(),
        KeyCode::Down | KeyCode::Char('j') => app.move_env_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_env_selection(-1),
        KeyCode::PageDown => app.scroll_env_preview(10),
        KeyCode::PageUp => app.scroll_env_preview(-10),
        KeyCode::Home => app.environment.preview_scroll = 0,
        KeyCode::End => app.environment.preview_scroll = u16::MAX,
        KeyCode::Enter => app.activate_selected_env(),
        KeyCode::Char('d') | KeyCode::Char('D') => app.deactivate_env(),
        _ => {}
    }
}
