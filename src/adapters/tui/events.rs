use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, HistoryFocus, HistoryView, Screen};

pub(crate) fn handle_key_event(app: &mut App, key: KeyEvent) {
    // ── Prefix handling (Ctrl+/ tmux-style) ──────────────────────
    // Ctrl+/ sets prefix_pending; the NEXT key is dispatched as a
    // global navigation command regardless of which screen is active.
    if app.prefix_pending {
        app.prefix_pending = false;
        handle_prefix_command(app, key);
        return;
    }
    // Ctrl+/ produces different key events depending on the terminal
    // and keyboard layout:
    //   • Legacy xterm/VTE: ASCII 0x1F → Char('\x1f'), no modifiers
    //   • ABNT2 / layouts where / shares the 7 key: Char('7') + CONTROL
    //   • Enhanced keyboard protocol (kitty): Char('/') + CONTROL
    let is_prefix = matches!(key.code, KeyCode::Char('\x1f'))
        || (matches!(key.code, KeyCode::Char('7') | KeyCode::Char('/'))
            && key.modifiers.contains(KeyModifiers::CONTROL));
    if is_prefix {
        app.prefix_pending = true;
        return;
    }

    // ── Per-screen direct keys ───────────────────────────────────
    match app.screen {
        Screen::ScriptSelect => handle_list_key(app, key),
        Screen::Search => handle_search_key(app, key),
        Screen::Environments => handle_envs_key(app, key),
        Screen::FieldInput => handle_input_key(app, key),
        Screen::History => handle_history_key(app, key),
        Screen::Running => {}
        Screen::RunResult => handle_run_result_key(app, key),
        Screen::Error => handle_error_key(app, key),
        Screen::Schedules => handle_schedules_key(app, key),
    }
}

/// Global commands dispatched after the prefix key (Ctrl+/).
fn handle_prefix_command(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => app.should_quit = true,
        KeyCode::Char('s') | KeyCode::Char('S') => app.enter_search(),
        KeyCode::Char('e') | KeyCode::Char('E') => app.enter_envs(),
        KeyCode::Char('h') | KeyCode::Char('H') => {
            app.screen = Screen::History;
            app.history.focus = HistoryFocus::List;
            app.reset_run_output_scroll();
        }
        KeyCode::Char('c') | KeyCode::Char('C') => app.enter_schedules(),
        _ => {}
    }
}

// ── ScriptSelect ─────────────────────────────────────────────────

fn handle_list_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
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
        KeyCode::Tab => app.activity_period = app.activity_period.next(),
        KeyCode::PageDown => app.scroll_schema_preview(5),
        KeyCode::PageUp => app.scroll_schema_preview(-5),
        KeyCode::Char('e') | KeyCode::Char('E') => {
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

// ── Search ───────────────────────────────────────────────────────

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

// ── FieldInput ───────────────────────────────────────────────────

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

// ── Error ────────────────────────────────────────────────────────

fn handle_error_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.should_quit = true,
        KeyCode::Enter => {
            app.error_message = None;
            app.screen = Screen::ScriptSelect;
        }
        _ => {}
    }
}

// ── History ──────────────────────────────────────────────────────

fn handle_history_key(app: &mut App, key: KeyEvent) {
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
            KeyCode::Esc => app.screen = Screen::ScriptSelect,
            KeyCode::Down | KeyCode::Char('j') => app.move_history_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_history_selection(-1),
            KeyCode::Enter | KeyCode::Right => {
                app.history.focus = HistoryFocus::Output;
                app.reset_run_output_scroll();
            }
            _ => {}
        },
        HistoryFocus::Output => match key.code {
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
        KeyCode::Esc => {
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

// ── RunResult ────────────────────────────────────────────────────

fn handle_run_result_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => app.screen = Screen::ScriptSelect,
        KeyCode::Down | KeyCode::Char('j') => app.scroll_run_output(1),
        KeyCode::Up | KeyCode::Char('k') => app.scroll_run_output(-1),
        KeyCode::PageDown => app.scroll_run_output(10),
        KeyCode::PageUp => app.scroll_run_output(-10),
        KeyCode::Home => app.run_output_scroll = 0,
        _ => {}
    }
}

// ── Environments ─────────────────────────────────────────────────

fn handle_envs_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.exit_envs(),
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

// ── Schedules ────────────────────────────────────────────────────

fn handle_schedules_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.screen = Screen::ScriptSelect,
        KeyCode::Char(' ') => app.toggle_selected_schedule(),
        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => app.enter_schedules(),
        KeyCode::Tab => app.activity_period = app.activity_period.next(),
        KeyCode::Down | KeyCode::Char('j') => app.move_schedules_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_schedules_selection(-1),
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

    fn prefix() -> KeyEvent {
        // Ctrl+/ sends ASCII 0x1F under the legacy terminal protocol.
        key(KeyCode::Char('\x1f'))
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

    // ── Prefix key tests ─────────────────────────────────────────

    #[test]
    fn prefix_q_quits() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, prefix());
        assert!(app.prefix_pending);
        handle_key_event(&mut app, key(KeyCode::Char('q')));
        assert!(app.should_quit);
        assert!(!app.prefix_pending);
    }

    #[test]
    fn prefix_s_enters_search() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, prefix());
        handle_key_event(&mut app, key(KeyCode::Char('s')));
        assert_eq!(app.screen, Screen::Search);
    }

    #[test]
    fn prefix_e_enters_envs() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, prefix());
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert_eq!(app.screen, Screen::Environments);
    }

    #[test]
    fn prefix_h_enters_history() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, prefix());
        handle_key_event(&mut app, key(KeyCode::Char('h')));
        assert_eq!(app.screen, Screen::History);
    }

    #[test]
    fn prefix_c_enters_schedules() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, prefix());
        handle_key_event(&mut app, key(KeyCode::Char('c')));
        assert_eq!(app.screen, Screen::Schedules);
    }

    #[test]
    fn unknown_prefix_key_cancels_silently() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, prefix());
        handle_key_event(&mut app, key(KeyCode::Char('z')));
        assert!(!app.prefix_pending);
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn prefix_works_from_any_screen() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        handle_key_event(&mut app, prefix());
        handle_key_event(&mut app, key(KeyCode::Char('s')));
        assert_eq!(app.screen, Screen::Search);
    }

    // ── Direct key tests (within-screen) ─────────────────────────

    #[test]
    fn j_moves_selection_down() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        assert_eq!(app.navigation.selection, 0);
        handle_key_event(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.navigation.selection, 1);
    }

    #[test]
    fn k_moves_selection_up() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.move_selection(1);
        handle_key_event(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.navigation.selection, 0);
    }

    #[test]
    fn esc_from_search_returns_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn search_typing_appends_chars() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        handle_key_event(&mut app, key(KeyCode::Char('d')));
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert_eq!(app.search.query, "de");
    }

    #[test]
    fn search_backspace_pops_char() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Search;
        app.search.query = "abc".to_string();
        handle_key_event(&mut app, key(KeyCode::Backspace));
        assert_eq!(app.search.query, "ab");
    }

    #[test]
    fn error_enter_returns_to_scripts() {
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
    fn error_esc_quits() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Error;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn history_tab_toggles_view() {
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
    fn history_esc_returns_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn history_list_enter_focuses_output() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.focus = HistoryFocus::List;
        handle_key_event(&mut app, key(KeyCode::Enter));
        assert_eq!(app.history.focus, HistoryFocus::Output);
    }

    #[test]
    fn history_output_esc_returns_to_list() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::History;
        app.history.focus = HistoryFocus::Output;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.history.focus, HistoryFocus::List);
    }

    #[test]
    fn run_result_esc_returns() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::RunResult;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn field_input_esc_returns() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn field_input_char_appends() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        app.field_input.fields = vec![crate::domain::Field {
            name: "target".to_string(),
            prompt: None,
            kind: "string".to_string(),
            order: Some(0),
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
    fn running_key_is_noop() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::Running;
        let screen_before = app.screen;
        handle_key_event(&mut app, key(KeyCode::Char('x')));
        assert_eq!(app.screen, screen_before);
        assert!(!app.should_quit);
    }

    #[test]
    fn esc_quits_at_workspace_root() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn esc_collapses_dashboard_expansion_first() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.script_dashboard_expanded = true;
        handle_key_event(&mut app, key(KeyCode::Esc));
        assert!(!app.script_dashboard_expanded);
        assert!(!app.should_quit);
    }

    #[test]
    fn e_toggles_script_dashboard_only_for_script() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.move_selection(1);
        assert!(!app.script_dashboard_expanded);
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert!(app.script_dashboard_expanded);
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert!(!app.script_dashboard_expanded);
    }

    #[test]
    fn e_over_directory_is_noop() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Char('e')));
        assert!(!app.script_dashboard_expanded);
    }

    #[test]
    fn r_refreshes_entries() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Char('r')));
        handle_key_event(&mut app, key(KeyCode::F(5)));
        handle_key_event(&mut app, key(KeyCode::Char('i')));
        handle_key_event(&mut app, key(KeyCode::F(6)));
    }

    #[test]
    fn backspace_navigates_up() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        handle_key_event(&mut app, key(KeyCode::Backspace));
    }

    #[test]
    fn search_navigation_keys() {
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
    fn field_input_navigation_keys() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        app.field_input.fields = vec![
            crate::domain::Field {
                name: "a".to_string(),
                prompt: None,
                kind: "string".to_string(),
                order: Some(0),
                required: None,
                default: None,
                choices: None,
                arg: None,
            },
            crate::domain::Field {
                name: "b".to_string(),
                prompt: None,
                kind: "string".to_string(),
                order: Some(1),
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
    fn history_list_navigation_and_right() {
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
    fn history_output_scroll_and_paging_keys() {
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
    fn history_dashboards_navigation_and_esc() {
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
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn run_result_enter_returns_to_scripts() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::RunResult;
        handle_key_event(&mut app, key(KeyCode::Enter));
        assert_eq!(app.screen, Screen::ScriptSelect);
    }

    #[test]
    fn run_result_scroll_keys() {
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
    fn envs_screen_keys_dispatch() {
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
        handle_key_event(&mut app, key(KeyCode::Esc));
    }

    #[test]
    fn field_input_enter_submits_form() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let mut app = setup_app(&tmp, &svc);
        app.screen = Screen::FieldInput;
        handle_key_event(&mut app, key(KeyCode::Enter));
    }
}
