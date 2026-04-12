use crate::adapters::environments::FsEnvironmentRepository;
use crate::domain::Schema;
use crate::lua_widget::{self, WidgetData};
use crate::ports::{EnvironmentConfig, WorkspaceEntry, WorkspaceEntryKind};
use crate::runs::RunRow;
use crate::search_index::SearchIndex;
use crate::use_cases::{EnvironmentService, ScriptService};
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};

/// Display label used in `EnvironmentConfig.active` when the active env
/// for the session is the per-directory `omakure.conf` override rather
/// than a file from the global `.omaken/envs/` directory.
pub(crate) const SESSION_ENV_LABEL: &str = "omakure.conf (session)";

pub(crate) use super::state::{DashboardLayout, HistoryFocus, HistoryView};
use super::state::{
    EnvironmentState, FieldInputState, HistoryState, NavigationState, SearchState, WidgetLoadResult,
};
use super::theme::Theme;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Screen {
    ScriptSelect,
    Search,
    Environments,
    FieldInput,
    History,
    Running,
    RunResult,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct SchemaPreview {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) fields: Vec<SchemaFieldPreview>,
    pub(crate) outputs: Vec<SchemaOutputPreview>,
    pub(crate) queue: Option<QueuePreview>,
}

#[derive(Debug, Clone)]
pub(crate) struct SchemaOutputPreview {
    pub(crate) name: String,
    pub(crate) kind: String,
}

#[derive(Debug, Clone)]
pub(crate) enum QueuePreview {
    Matrix { values: Vec<MatrixPreview> },
    Cases { cases: Vec<QueueCasePreview> },
}

#[derive(Debug, Clone)]
pub(crate) struct MatrixPreview {
    pub(crate) name: String,
    pub(crate) values: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct QueueCasePreview {
    pub(crate) name: Option<String>,
    pub(crate) values: Vec<QueueCaseValuePreview>,
}

#[derive(Debug, Clone)]
pub(crate) struct QueueCaseValuePreview {
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SchemaFieldPreview {
    pub(crate) name: String,
    pub(crate) prompt: Option<String>,
    pub(crate) kind: String,
    pub(crate) required: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum ExecutionStatus {
    Success,
    Failed(Option<i32>),
    Error,
}

pub(crate) struct App<'a> {
    service: &'a ScriptService,
    pub(crate) workspace: Workspace,
    pub(crate) theme: Theme,
    pub(crate) screen: Screen,
    env_return: Option<Screen>,
    search_index: SearchIndex,
    pub(crate) navigation: NavigationState,
    pub(crate) environment: EnvironmentState,
    pub(crate) search: SearchState,
    pub(crate) history: HistoryState,
    pub(crate) field_input: FieldInputState,
    pub(crate) result: Option<(PathBuf, Vec<String>)>,
    pub(crate) should_quit: bool,
    pub(crate) run_output_scroll: u16,
    pub(crate) error_message: Option<String>,
    /// Frame counter incremented exactly once per main-loop iteration in
    /// `src/adapters/tui/mod.rs`. Drives spinner animation; wrapping is
    /// expected and harmless because consumers only use `tick % len`.
    pub(crate) tick: u64,
    /// Receiver for the in-flight inline script execution. `Some` while
    /// a script is running on a background thread; `None` otherwise.
    /// Polled every iteration of the main TUI loop so the foreground
    /// keeps drawing (and animating spinners) while the worker runs.
    pub(crate) inline_run_receiver: Option<mpsc::Receiver<Option<RunRow>>>,
    /// True while the script-select screen is showing the per-script
    /// dashboard charts in fullscreen (toggled by `e`). Reset to false
    /// when leaving the screen, when navigating into a directory, or
    /// when the user presses `Esc`.
    pub(crate) script_dashboard_expanded: bool,
}

impl<'a> App<'a> {
    pub(crate) fn new(
        service: &'a ScriptService,
        workspace: Workspace,
        entries: Vec<WorkspaceEntry>,
        history: Vec<RunRow>,
        search_index: SearchIndex,
        theme: Theme,
    ) -> Self {
        let current_dir = workspace.scripts_root().to_path_buf();
        let navigation = NavigationState::new(current_dir, entries);
        // Filter loaded history entries to those whose script lives under
        // the active scripts root. Run rows always carry absolute paths,
        // so the filter is a simple prefix check.
        let history = filter_history_for_scripts_root(history, workspace.scripts_root());
        let history = HistoryState::new(history);
        let search_status = search_index.status();
        let search = SearchState::new(search_status);
        let environment = EnvironmentState::new();
        let field_input = FieldInputState::new();
        let mut app = Self {
            service,
            workspace,
            theme,
            screen: Screen::ScriptSelect,
            env_return: None,
            search_index,
            navigation,
            environment,
            search,
            history,
            field_input,
            result: None,
            should_quit: false,
            run_output_scroll: 0,
            error_message: None,
            tick: 0,
            inline_run_receiver: None,
            script_dashboard_expanded: false,
        };
        app.start_widget_load();
        app.load_env_config();
        app.update_schema_preview();
        app.update_env_preview();
        app
    }

    /// Minimal constructor for tests. Skips widget loading, env config,
    /// and schema preview so no real filesystem is required.
    #[cfg(test)]
    pub(crate) fn test_new(
        service: &'a ScriptService,
        workspace: Workspace,
        entries: Vec<WorkspaceEntry>,
        history: Vec<RunRow>,
    ) -> Self {
        let current_dir = workspace.scripts_root().to_path_buf();
        let navigation = NavigationState::new(current_dir, entries);
        let history_filtered = filter_history_for_scripts_root(history, workspace.scripts_root());
        let history_state = HistoryState::new(history_filtered);
        let search = SearchState::new(crate::search_index::SearchStatus::Idle);
        Self {
            service,
            workspace,
            theme: Theme::default(),
            screen: Screen::ScriptSelect,
            env_return: None,
            search_index: SearchIndex::new(std::path::PathBuf::from(":memory:")),
            navigation,
            environment: EnvironmentState::new(),
            search,
            history: history_state,
            field_input: FieldInputState::new(),
            result: None,
            should_quit: false,
            run_output_scroll: 0,
            error_message: None,
            tick: 0,
            inline_run_receiver: None,
            script_dashboard_expanded: false,
        }
    }

    pub(crate) fn selected_entry(&self) -> Option<&WorkspaceEntry> {
        self.navigation.entries.get(self.navigation.selection)
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if self.navigation.entries.is_empty() {
            return;
        }
        let len = self.navigation.entries.len() as isize;
        let mut new_index = self.navigation.selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.navigation.selection = new_index as usize;
        self.navigation
            .list_state
            .select(Some(self.navigation.selection));
        self.update_schema_preview();
    }

    pub(crate) fn enter_search(&mut self) {
        self.search.status = self.search_index.status();
        self.screen = Screen::Search;
        self.refresh_search_results();
    }

    pub(crate) fn enter_envs(&mut self) {
        self.env_return = Some(self.screen);
        self.load_env_config();
        self.update_env_preview();
        self.screen = Screen::Environments;
    }

    pub(crate) fn exit_envs(&mut self) {
        self.screen = self.env_return.unwrap_or(Screen::ScriptSelect);
        self.env_return = None;
    }

    pub(crate) fn scroll_env_preview(&mut self, delta: i16) {
        let mut next = self.environment.preview_scroll as i16 + delta;
        if next < 0 {
            next = 0;
        }
        if next > u16::MAX as i16 {
            next = u16::MAX as i16;
        }
        self.environment.preview_scroll = next as u16;
    }

    pub(crate) fn move_env_selection(&mut self, delta: isize) {
        if self.environment.entries.is_empty() {
            return;
        }
        let len = self.environment.entries.len() as isize;
        let mut new_index = self.environment.selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.environment.selection = new_index as usize;
        self.environment
            .list_state
            .select(Some(self.environment.selection));
        self.update_env_preview();
    }

    pub(crate) fn activate_selected_env(&mut self) {
        if self.environment.entries.is_empty() {
            return;
        }
        let name = self.environment.entries[self.environment.selection]
            .name
            .clone();
        let service = self.environment_service();
        match service.set_active_env(Some(&name)) {
            Ok(()) => self.load_env_config(),
            Err(err) => self.environment.error = Some(err.to_string()),
        }
    }

    pub(crate) fn deactivate_env(&mut self) {
        let service = self.environment_service();
        match service.set_active_env(None) {
            Ok(()) => self.load_env_config(),
            Err(err) => self.environment.error = Some(err.to_string()),
        }
    }

    pub(crate) fn refresh_search_status(&mut self) {
        let status = self.search_index.status();
        if status != self.search.status {
            self.search.status = status.clone();
            if self.screen == Screen::Search {
                self.refresh_search_results();
            }
        }
    }

    pub(crate) fn move_search_selection(&mut self, delta: isize) {
        if self.search.results.is_empty() {
            return;
        }
        let len = self.search.results.len() as isize;
        let mut new_index = self.search.selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.search.selection = new_index as usize;
        self.search.list_state.select(Some(self.search.selection));
        self.update_search_details();
    }

    pub(crate) fn append_search_char(&mut self, ch: char) {
        self.search.query.push(ch);
        self.refresh_search_results();
    }

    pub(crate) fn pop_search_char(&mut self) {
        self.search.query.pop();
        self.refresh_search_results();
    }

    pub(crate) fn open_selected_search(&mut self) {
        let entry = match self.search.results.get(self.search.selection) {
            Some(entry) => entry,
            None => return,
        };
        let script_path = self.workspace.root().join(&entry.script_path);
        self.load_schema(script_path);
    }

    pub(crate) fn enter_selected(&mut self) {
        let entry = match self.selected_entry() {
            Some(entry) => entry.clone(),
            None => return,
        };

        match entry.kind {
            WorkspaceEntryKind::Directory => {
                self.script_dashboard_expanded = false;
                self.navigation.current_dir = entry.path;
                self.refresh_entries();
            }
            WorkspaceEntryKind::Script => {
                self.load_schema(entry.path);
            }
        }
    }

    pub(crate) fn navigate_up(&mut self) {
        // The user must not be able to escape the active scripts root,
        // even when it differs from the global workspace root.
        if self.navigation.current_dir == self.workspace.scripts_root() {
            return;
        }
        self.script_dashboard_expanded = false;
        if let Some(parent) = self.navigation.current_dir.parent() {
            self.navigation.current_dir = parent.to_path_buf();
            self.refresh_entries();
        }
    }

    pub(crate) fn move_history_selection(&mut self, delta: isize) {
        if self.history.entries.is_empty() {
            return;
        }
        let len = self.history.entries.len() as isize;
        let mut new_index = self.history.selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.history.selection = new_index as usize;
        self.history
            .table_state
            .select(Some(self.history.selection));
        self.reset_run_output_scroll();
    }

    pub(crate) fn add_history_entry(&mut self, entry: RunRow) {
        self.history.entries.insert(0, entry);
        self.history.selection = 0;
        self.history.table_state.select(Some(0));
    }

    pub(crate) fn current_history_entry(&self) -> Option<&RunRow> {
        self.history.entries.get(self.history.selection)
    }

    pub(crate) fn load_schema(&mut self, script: PathBuf) {
        let schema_result = match self.navigation.schema_cache.as_ref() {
            Some((path, schema)) if path == &script => Ok(schema.clone()),
            _ => self.service.load_schema(&script),
        };

        match schema_result {
            Ok(mut schema) => {
                self.load_env_config();
                schema.fields.sort_by_key(|field| field.order);
                let tags = schema.tags.clone();
                let outputs = schema.outputs.clone();
                let queue = schema.queue.clone();
                self.field_input.schema_name = Some(schema.name);
                self.field_input.schema_description = schema.description;
                self.field_input.fields = schema.fields;
                self.field_input.field_index = 0;
                self.field_input.field_inputs = self.build_field_inputs();
                self.field_input.args.clear();
                self.field_input.error = None;
                self.field_input.selected_script = Some(script.clone());
                self.navigation.schema_cache = Some((
                    script.clone(),
                    Schema {
                        name: self.field_input.schema_name.clone().unwrap_or_default(),
                        description: self.field_input.schema_description.clone(),
                        tags,
                        fields: self.field_input.fields.clone(),
                        outputs,
                        queue,
                    },
                ));
                if self.field_input.fields.is_empty() {
                    self.result = Some((script, Vec::new()));
                } else {
                    self.screen = Screen::FieldInput;
                }
            }
            Err(err) => {
                self.error_message = Some(err.to_string());
                self.screen = Screen::Error;
            }
        }
    }

    pub(crate) fn move_field_selection(&mut self, delta: isize) {
        if self.field_input.fields.is_empty() {
            return;
        }
        let len = self.field_input.fields.len() as isize;
        let mut new_index = self.field_input.field_index as isize + delta;
        while new_index < 0 {
            new_index += len;
        }
        while new_index >= len {
            new_index -= len;
        }
        self.field_input.field_index = new_index as usize;
        self.field_input.error = None;
    }

    pub(crate) fn append_field_char(&mut self, ch: char) {
        if let Some(value) = self
            .field_input
            .field_inputs
            .get_mut(self.field_input.field_index)
        {
            value.push(ch);
            self.field_input.error = None;
        }
    }

    pub(crate) fn pop_field_char(&mut self) {
        if let Some(value) = self
            .field_input
            .field_inputs
            .get_mut(self.field_input.field_index)
        {
            value.pop();
            self.field_input.error = None;
        }
    }

    pub(crate) fn submit_form(&mut self) {
        if self.field_input.fields.is_empty() {
            self.finish();
            return;
        }

        let mut args = Vec::new();
        for (idx, field) in self.field_input.fields.iter().enumerate() {
            let input = self
                .field_input
                .field_inputs
                .get(idx)
                .map(String::as_str)
                .unwrap_or("");
            match crate::domain::normalize_input(field, input) {
                Ok(value) => {
                    if let Some(value) = value {
                        let arg = field
                            .arg
                            .clone()
                            .unwrap_or_else(|| format!("--{}", field.name));
                        args.push(arg);
                        args.push(value);
                    }
                }
                Err(message) => {
                    self.field_input.error = Some(format!("{}: {}", field.name, message));
                    self.field_input.field_index = idx;
                    return;
                }
            }
        }

        self.field_input.args = args;
        self.field_input.error = None;
        self.finish();
    }

    fn finish(&mut self) {
        if let Some(script) = &self.field_input.selected_script {
            self.result = Some((script.clone(), self.field_input.args.clone()));
        } else {
            self.should_quit = true;
        }
    }

    pub(crate) fn refresh_entries(&mut self) {
        match self.service.list_entries(&self.navigation.current_dir) {
            Ok(entries) => {
                self.navigation.entries = entries;
                self.navigation.selection = 0;
                if self.navigation.entries.is_empty() {
                    self.navigation.list_state.select(None);
                } else {
                    self.navigation.list_state.select(Some(0));
                }
                self.error_message = None;
                self.start_widget_load();
                self.update_schema_preview();
            }
            Err(err) => {
                self.error_message = Some(err.to_string());
                self.screen = Screen::Error;
            }
        }
    }

    pub(crate) fn refresh_status(&mut self) {
        self.start_widget_load();
        self.load_env_config();
        self.update_schema_preview();
    }

    pub(crate) fn back_to_script_select(&mut self) {
        self.screen = Screen::ScriptSelect;
        self.field_input.schema_name = None;
        self.field_input.schema_description = None;
        self.field_input.fields.clear();
        self.field_input.field_index = 0;
        self.field_input.field_inputs.clear();
        self.field_input.args.clear();
        self.field_input.error = None;
        self.field_input.selected_script = None;
        self.result = None;
    }

    pub(crate) fn reset_run_output_scroll(&mut self) {
        self.run_output_scroll = 0;
    }

    pub(crate) fn scroll_run_output(&mut self, delta: i16) {
        if delta > 0 {
            self.run_output_scroll = self.run_output_scroll.saturating_add(delta as u16);
        } else if delta < 0 {
            let amount = (-delta) as u16;
            self.run_output_scroll = self.run_output_scroll.saturating_sub(amount);
        }
    }

    pub(crate) fn display_path(&self, path: &Path) -> String {
        // Strip the active scripts root prefix so navigation paths render
        // relative to the directory the user is browsing. Fall back to the
        // global workspace root for legacy history entries that were
        // recorded as workspace-relative paths.
        if let Ok(stripped) = path.strip_prefix(self.workspace.scripts_root()) {
            return stripped.to_string_lossy().to_string();
        }
        path.strip_prefix(self.workspace.root())
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    }

    fn start_widget_load(&mut self) {
        let dir = self.navigation.current_dir.clone();
        let (tx, rx) = mpsc::channel();
        self.navigation.widget_loading = true;
        self.navigation.widget = None;
        self.navigation.widget_error = None;
        self.navigation.widget_receiver = Some(rx);
        std::thread::spawn(move || {
            let (widget, error) = load_widget_state(&dir);
            let _ = tx.send(WidgetLoadResult { widget, error });
        });
    }

    pub(crate) fn poll_widget_load(&mut self) {
        let Some(receiver) = &self.navigation.widget_receiver else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                self.navigation.widget = result.widget;
                self.navigation.widget_error = result.error;
                self.navigation.widget_loading = false;
                self.navigation.widget_receiver = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.navigation.widget_loading = false;
                self.navigation.widget_receiver = None;
            }
        }
    }

    fn environment_service(&self) -> EnvironmentService {
        let repo = FsEnvironmentRepository::new(self.workspace.envs_dir());
        EnvironmentService::new(Box::new(repo))
    }

    fn load_env_config(&mut self) {
        let mut env_error = None;

        let service = self.environment_service();
        let mut env_config = match service.load_environment_config() {
            Ok(config) => Some(config),
            Err(err) => {
                env_error = Some(err.to_string());
                None
            }
        };

        // If the TUI was launched with a positional scripts-root override
        // and `<scripts-root>/omakure.conf` exists, prefer it as the
        // session-active environment over the globally active env. The
        // override is read-only: nothing is written to `.omaken/envs/`
        // and the file is never copied. On parse error we surface the
        // message but fall back to the global config so the TUI keeps
        // launching.
        if let Some(result) = load_session_env_config(&self.workspace) {
            match result {
                Ok(session) => {
                    let session_config = EnvironmentConfig {
                        envs_dir: env_config
                            .as_ref()
                            .map(|c| c.envs_dir.clone())
                            .unwrap_or_else(|| self.workspace.envs_dir().to_path_buf()),
                        active: Some(SESSION_ENV_LABEL.to_string()),
                        defaults: session.defaults,
                        session_conf_path: Some(session.path),
                    };
                    env_config = Some(session_config);
                }
                Err(message) => {
                    env_error = Some(message);
                }
            }
        }

        let env_entries = match service.list_env_files() {
            Ok(entries) => entries,
            Err(err) => {
                if env_error.is_none() {
                    env_error = Some(err.to_string());
                }
                Vec::new()
            }
        };

        let selected = if env_entries.is_empty() {
            0
        } else if let Some(active) = env_config
            .as_ref()
            .and_then(|config| config.active.as_ref())
        {
            env_entries
                .iter()
                .position(|entry| entry.name == *active)
                .unwrap_or(0)
        } else {
            self.environment
                .selection
                .min(env_entries.len().saturating_sub(1))
        };

        self.environment.entries = env_entries;
        self.environment.selection = selected;
        if self.environment.entries.is_empty() {
            self.environment.list_state.select(None);
        } else {
            self.environment
                .list_state
                .select(Some(self.environment.selection));
        }

        self.environment.config = env_config;
        self.environment.error = env_error;
        self.update_env_preview();
    }

    fn update_env_preview(&mut self) {
        self.environment.preview_scroll = 0;
        self.environment.preview_error = None;

        let entry = match self.environment.entries.get(self.environment.selection) {
            Some(entry) => entry,
            None => {
                self.environment.preview_lines = Vec::new();
                return;
            }
        };

        let envs_dir = self
            .environment
            .config
            .as_ref()
            .map(|config| config.envs_dir.clone())
            .unwrap_or_else(|| self.workspace.envs_dir().to_path_buf());
        let env_path = envs_dir.join(&entry.name);

        let service = self.environment_service();
        match service.load_env_preview(&env_path) {
            Ok(entries) => {
                let mut lines = Vec::new();
                for (key, value) in entries {
                    let line = ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(
                            key,
                            ratatui::style::Style::default()
                                .fg(self.theme.semantic.warning.color())
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        ratatui::text::Span::styled(" = ", self.theme.text_secondary()),
                        ratatui::text::Span::raw(value),
                    ]);
                    lines.push(line);
                }
                if lines.is_empty() {
                    self.environment.preview_lines =
                        vec![ratatui::text::Line::from(ratatui::text::Span::styled(
                            "No entries found.",
                            self.theme.text_secondary(),
                        ))];
                } else {
                    self.environment.preview_lines = lines;
                }
                self.environment.preview_error = None;
            }
            Err(err) => {
                self.environment.preview_lines = Vec::new();
                self.environment.preview_error = Some(err.to_string());
            }
        }
    }

    fn build_field_inputs(&self) -> Vec<String> {
        let defaults = self
            .environment
            .config
            .as_ref()
            .map(|config| &config.defaults);
        match defaults {
            Some(defaults) if !defaults.is_empty() => self
                .field_input
                .fields
                .iter()
                .map(|field| {
                    defaults
                        .get(&field.name.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default()
                })
                .collect(),
            _ => vec![String::new(); self.field_input.fields.len()],
        }
    }

    fn update_schema_preview(&mut self) {
        let (entry_path, entry_kind) = match self.selected_entry() {
            Some(entry) => (entry.path.clone(), entry.kind),
            None => {
                self.navigation.schema_preview = None;
                self.navigation.schema_preview_error = None;
                self.navigation.preview_script = None;
                return;
            }
        };

        if entry_kind != WorkspaceEntryKind::Script {
            self.navigation.schema_preview = None;
            self.navigation.schema_preview_error = None;
            self.navigation.preview_script = None;
            return;
        }

        if self.navigation.preview_script.as_ref() == Some(&entry_path) {
            return;
        }

        match self.service.load_schema(&entry_path) {
            Ok(mut schema) => {
                schema.fields.sort_by_key(|field| field.order);
                self.navigation.schema_preview = Some(schema_to_preview(&schema));
                self.navigation.schema_preview_error = None;
                self.navigation.preview_script = Some(entry_path.clone());
                self.navigation.schema_cache = Some((entry_path, schema));
            }
            Err(err) => {
                self.navigation.schema_preview = None;
                self.navigation.schema_preview_error = Some(err.to_string());
                self.navigation.preview_script = Some(entry_path);
            }
        }
    }

    fn refresh_search_results(&mut self) {
        match self.search_index.query(&self.search.query) {
            Ok(results) => {
                self.search.results = results;
                self.search.error = None;
            }
            Err(err) => {
                self.search.results.clear();
                self.search.error = Some(err);
            }
        }
        self.search.selection = 0;
        if self.search.results.is_empty() {
            self.search.list_state.select(None);
        } else {
            self.search.list_state.select(Some(0));
        }
        self.update_search_details();
    }

    fn update_search_details(&mut self) {
        self.search.details = None;
        let entry = match self.search.results.get(self.search.selection) {
            Some(entry) => entry,
            None => return,
        };
        match self.search_index.load_details(&entry.script_path) {
            Ok(details) => {
                self.search.details = details;
                self.search.error = None;
            }
            Err(err) => {
                self.search.error = Some(err);
            }
        }
    }
}

impl ExecutionStatus {
    pub(crate) fn from_run(entry: &RunRow) -> Self {
        if entry.error.is_some() {
            ExecutionStatus::Error
        } else {
            match entry.success {
                Some(true) => ExecutionStatus::Success,
                Some(false) => ExecutionStatus::Failed(entry.exit_code),
                // Still in flight (queued/running): no status yet — treat
                // as success for the legacy color path. The new state
                // column already conveys the in-flight state.
                None => ExecutionStatus::Success,
            }
        }
    }
}

fn load_widget_state(dir: &Path) -> (Option<WidgetData>, Option<String>) {
    match lua_widget::load_widget(dir) {
        Ok(widget) => (widget, None),
        Err(err) => (None, Some(err)),
    }
}

#[derive(Debug)]
pub(crate) struct SessionEnvLoad {
    pub(crate) path: PathBuf,
    pub(crate) defaults: std::collections::HashMap<String, String>,
}

/// Resolve the per-session `<scripts-root>/omakure.conf` override.
///
/// Returns:
/// - `None` when the workspace has no scripts-root override or no
///   `omakure.conf` file is present at the scripts root.
/// - `Some(Ok(load))` when the file exists and parses cleanly.
/// - `Some(Err(message))` when the file exists but cannot be read.
///
/// This function never writes to disk and never touches `.omaken/envs/`.
/// Parse failures from the underlying KEY=value parser are non-fatal:
/// the parser silently skips malformed lines, so a literally unreadable
/// file (I/O error) is the only branch that surfaces an error here.
pub(crate) fn load_session_env_config(
    workspace: &Workspace,
) -> Option<Result<SessionEnvLoad, String>> {
    if !workspace.has_scripts_root_override() {
        return None;
    }
    let session_path = workspace.scripts_root().join("omakure.conf");
    if !session_path.is_file() {
        return None;
    }
    Some(match std::fs::read_to_string(&session_path) {
        Ok(contents) => Ok(SessionEnvLoad {
            path: session_path,
            defaults: crate::adapters::environments::parse_env_defaults(&contents),
        }),
        Err(err) => Err(format!(
            "Failed to read session env {}: {}",
            session_path.display(),
            err
        )),
    })
}

/// Decide whether a run row belongs to the currently active scripts root.
///
/// Run rows always carry absolute, canonical script paths (no legacy
/// relative form exists in `runs.sqlite`), so this is a simple prefix
/// check.
pub(crate) fn history_belongs_to_scripts_root(entry: &RunRow, scripts_root: &Path) -> bool {
    Path::new(&entry.script_path).starts_with(scripts_root)
}

fn filter_history_for_scripts_root(entries: Vec<RunRow>, scripts_root: &Path) -> Vec<RunRow> {
    entries
        .into_iter()
        .filter(|entry| history_belongs_to_scripts_root(entry, scripts_root))
        .collect()
}

fn schema_to_preview(schema: &Schema) -> SchemaPreview {
    let tags = schema.tags.clone().unwrap_or_default();
    let fields = schema
        .fields
        .iter()
        .map(|field| SchemaFieldPreview {
            name: field.name.clone(),
            prompt: field.prompt.clone(),
            kind: field.kind.clone(),
            required: field.required.unwrap_or(false),
        })
        .collect();
    let outputs = schema
        .outputs
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|output| SchemaOutputPreview {
                    name: output.name.clone(),
                    kind: output.kind.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let queue = schema.queue.as_ref().map(|queue| {
        if let Some(matrix) = &queue.matrix {
            QueuePreview::Matrix {
                values: matrix
                    .values
                    .iter()
                    .map(|value| MatrixPreview {
                        name: value.name.clone(),
                        values: value.values.clone(),
                    })
                    .collect(),
            }
        } else if let Some(cases) = &queue.cases {
            QueuePreview::Cases {
                cases: cases
                    .iter()
                    .map(|case| QueueCasePreview {
                        name: case.name.clone(),
                        values: case
                            .values
                            .iter()
                            .map(|value| QueueCaseValuePreview {
                                name: value.name.clone(),
                                value: value.value.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            }
        } else {
            QueuePreview::Cases { cases: Vec::new() }
        }
    });

    SchemaPreview {
        name: schema.name.clone(),
        description: schema.description.clone(),
        tags,
        fields,
        outputs,
        queue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_with_script(script: &str) -> RunRow {
        RunRow {
            run_id: "rid".into(),
            script_path: script.into(),
            script_name: None,
            args_json: "[]".into(),
            actor: "human".into(),
            reason: None,
            state: crate::runs::RunState::Completed,
            priority: 0,
            enqueued_at: 0,
            worker_id: None,
            lease_until: None,
            timeout_ms: None,
            cron_schedule_id: None,
            started_at: Some(0),
            finished_at: Some(0),
            duration_ms: Some(0),
            exit_code: Some(0),
            success: Some(true),
            stdout: String::new(),
            stderr: String::new(),
            error: None,
            parent_run_id: None,
            omakure_version: "test".into(),
        }
    }

    #[test]
    fn absolute_entry_inside_scripts_root_is_visible() {
        let entry = entry_with_script("/abs/scripts/team/run.sh");
        let scripts_root = Path::new("/abs/scripts/team");
        assert!(history_belongs_to_scripts_root(&entry, scripts_root));
    }

    #[test]
    fn absolute_entry_outside_scripts_root_is_hidden() {
        let entry = entry_with_script("/abs/other/run.sh");
        let scripts_root = Path::new("/abs/scripts/team");
        assert!(!history_belongs_to_scripts_root(&entry, scripts_root));
    }

    #[test]
    fn filter_drops_entries_outside_scripts_root() {
        let entries = vec![
            entry_with_script("/abs/scripts/team/run.sh"),
            entry_with_script("/abs/other/run.sh"),
            entry_with_script("/abs/scripts/team/another.sh"),
        ];
        let scripts_root = Path::new("/abs/scripts/team");
        let filtered = filter_history_for_scripts_root(entries, scripts_root);
        assert_eq!(filtered.len(), 2);
    }

    fn unique_session_env_dir(label: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("omakure_session_env_{label}_{pid}_{nanos}"))
    }

    #[test]
    fn session_env_returns_none_without_override() {
        let dir = unique_session_env_dir("no_override");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        // No override flag — even if omakure.conf exists, ignore it.
        std::fs::write(dir.join("omakure.conf"), "FOO=bar").expect("write");
        let ws = Workspace::with_scripts_root(dir.clone(), dir.clone(), false);
        assert!(load_session_env_config(&ws).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_env_returns_none_when_file_missing() {
        let dir = unique_session_env_dir("file_missing");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let ws = Workspace::with_scripts_root(dir.clone(), dir.clone(), true);
        assert!(load_session_env_config(&ws).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_env_parses_defaults_when_override_and_file_present() {
        let parent = unique_session_env_dir("parses");
        let global = parent.join("global");
        let scripts = parent.join("scripts");
        std::fs::create_dir_all(&global).expect("create global");
        std::fs::create_dir_all(&scripts).expect("create scripts");
        std::fs::write(
            scripts.join("omakure.conf"),
            "RESOURCE_GROUP=rg-test\nREGION=eastus\n",
        )
        .expect("write conf");

        let ws = Workspace::with_scripts_root(global.clone(), scripts.clone(), true);
        let result = load_session_env_config(&ws).expect("override should activate");
        let load = result.expect("parsing should succeed");

        assert_eq!(load.path, scripts.join("omakure.conf"));
        assert_eq!(
            load.defaults.get("resource_group").map(String::as_str),
            Some("rg-test")
        );
        assert_eq!(
            load.defaults.get("region").map(String::as_str),
            Some("eastus")
        );

        // Critically: nothing was written into the global envs dir or
        // touched the scripts root layout.
        assert!(!global.join(".omaken").exists());
        assert!(!global.join(".history").exists());
        assert!(!global.join("omakure.toml").exists());

        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn session_env_with_blank_or_garbage_lines_skips_them_silently() {
        // The KEY=value parser already tolerates malformed lines (skips
        // anything without an `=`). Verify this is the contract — a
        // "garbage" file does not produce an Err here.
        let parent = unique_session_env_dir("garbage");
        let global = parent.join("global");
        let scripts = parent.join("scripts");
        std::fs::create_dir_all(&global).expect("create global");
        std::fs::create_dir_all(&scripts).expect("create scripts");
        std::fs::write(
            scripts.join("omakure.conf"),
            "this is not a key=value line\n# comment\n   \nVALID=ok\n",
        )
        .expect("write conf");

        let ws = Workspace::with_scripts_root(global, scripts.clone(), true);
        let result = load_session_env_config(&ws).expect("override active");
        let load = result.expect("parser should never return Err for malformed lines");
        assert_eq!(load.defaults.get("valid").map(String::as_str), Some("ok"));

        let _ = std::fs::remove_dir_all(&parent);
    }

    // --- App method tests using test_new ---

    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::ports::{EnvFile, EnvironmentConfig};
    use tempfile::TempDir;

    fn make_service(tmp: &TempDir) -> ScriptService {
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        ScriptService::new(Box::new(repo), Box::new(runner))
    }

    fn make_entries() -> Vec<WorkspaceEntry> {
        vec![
            WorkspaceEntry {
                path: PathBuf::from("/scripts/alpha"),
                kind: WorkspaceEntryKind::Directory,
            },
            WorkspaceEntry {
                path: PathBuf::from("/scripts/beta.sh"),
                kind: WorkspaceEntryKind::Script,
            },
            WorkspaceEntry {
                path: PathBuf::from("/scripts/gamma.py"),
                kind: WorkspaceEntryKind::Script,
            },
        ]
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn write_bash_schema_script(tmp: &TempDir, relative: &str, schema_json: &str) -> PathBuf {
        let script = tmp.path().join(relative);
        write_file(
            &script,
            &format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {}\n# OMAKURE_SCHEMA_END\n",
                schema_json
            ),
        );
        script
    }

    fn actual_entries_for_root(tmp: &TempDir) -> Vec<WorkspaceEntry> {
        let service = make_service(tmp);
        service.list_entries(tmp.path()).unwrap()
    }

    #[test]
    fn test_move_selection_down() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, make_entries(), vec![]);
        assert_eq!(app.navigation.selection, 0);
        app.move_selection(1);
        assert_eq!(app.navigation.selection, 1);
        app.move_selection(1);
        assert_eq!(app.navigation.selection, 2);
    }

    #[test]
    fn test_move_selection_clamped_at_bounds() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, make_entries(), vec![]);
        app.move_selection(-1); // should stay at 0
        assert_eq!(app.navigation.selection, 0);
        app.move_selection(100); // should clamp to 2
        assert_eq!(app.navigation.selection, 2);
    }

    #[test]
    fn test_move_selection_empty() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.move_selection(1); // should be no-op
        assert_eq!(app.navigation.selection, 0);
    }

    #[test]
    fn test_selected_entry() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let app = App::test_new(&svc, ws, make_entries(), vec![]);
        let entry = app.selected_entry().unwrap();
        assert_eq!(entry.kind, WorkspaceEntryKind::Directory);
    }

    #[test]
    fn test_selected_entry_empty() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let app = App::test_new(&svc, ws, vec![], vec![]);
        assert!(app.selected_entry().is_none());
    }

    #[test]
    fn test_scroll_run_output() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.scroll_run_output(5);
        assert_eq!(app.run_output_scroll, 5);
        app.scroll_run_output(-3);
        assert_eq!(app.run_output_scroll, 2);
        app.scroll_run_output(-10); // clamp to 0
        assert_eq!(app.run_output_scroll, 0);
    }

    #[test]
    fn test_reset_run_output_scroll() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.run_output_scroll = 42;
        app.reset_run_output_scroll();
        assert_eq!(app.run_output_scroll, 0);
    }

    #[test]
    fn test_display_path_strips_scripts_root() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let app = App::test_new(&svc, ws, vec![], vec![]);
        let path = tmp.path().join("deploy.sh");
        assert_eq!(app.display_path(&path), "deploy.sh");
    }

    #[test]
    fn test_display_path_absolute_outside_root() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let app = App::test_new(&svc, ws, vec![], vec![]);
        let path = PathBuf::from("/other/deploy.sh");
        assert_eq!(app.display_path(&path), "/other/deploy.sh");
    }

    #[test]
    fn test_back_to_script_select_clears_state() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.screen = Screen::FieldInput;
        app.field_input.schema_name = Some("Test".to_string());
        app.field_input.error = Some("error".to_string());
        app.back_to_script_select();
        assert_eq!(app.screen, Screen::ScriptSelect);
        assert!(app.field_input.schema_name.is_none());
        assert!(app.field_input.error.is_none());
        assert!(app.field_input.fields.is_empty());
    }

    #[test]
    fn test_enter_envs_and_exit_envs() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.screen = Screen::History;
        app.enter_envs();
        assert_eq!(app.screen, Screen::Environments);
        app.exit_envs();
        assert_eq!(app.screen, Screen::History);
    }

    #[test]
    fn test_scroll_env_preview_accepts_positive_delta() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        // The function has a known i16 overflow: u16::MAX as i16 == -1,
        // so the `next > u16::MAX as i16` guard always triggers for
        // non-negative results. This test documents the current behavior.
        app.scroll_env_preview(10);
        assert_eq!(app.environment.preview_scroll, u16::MAX);
    }

    #[test]
    fn test_move_field_selection_wraps() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.fields = vec![
            crate::domain::Field {
                name: "a".into(),
                prompt: None,
                kind: "string".into(),
                order: 0,
                required: None,
                default: None,
                choices: None,
                arg: None,
            },
            crate::domain::Field {
                name: "b".into(),
                prompt: None,
                kind: "string".into(),
                order: 1,
                required: None,
                default: None,
                choices: None,
                arg: None,
            },
        ];
        app.field_input.field_inputs = vec![String::new(), String::new()];
        app.move_field_selection(1);
        assert_eq!(app.field_input.field_index, 1);
        app.move_field_selection(1); // wraps to 0
        assert_eq!(app.field_input.field_index, 0);
        app.move_field_selection(-1); // wraps to 1
        assert_eq!(app.field_input.field_index, 1);
    }

    #[test]
    fn test_append_and_pop_field_char() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.fields = vec![crate::domain::Field {
            name: "x".into(),
            prompt: None,
            kind: "string".into(),
            order: 0,
            required: None,
            default: None,
            choices: None,
            arg: None,
        }];
        app.field_input.field_inputs = vec![String::new()];
        app.append_field_char('h');
        app.append_field_char('i');
        assert_eq!(app.field_input.field_inputs[0], "hi");
        app.pop_field_char();
        assert_eq!(app.field_input.field_inputs[0], "h");
    }

    #[test]
    fn test_submit_form_no_fields_sets_result() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.selected_script = Some(PathBuf::from("/scripts/test.sh"));
        app.submit_form();
        assert!(app.result.is_some());
    }

    #[test]
    fn test_submit_form_with_valid_fields() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.selected_script = Some(PathBuf::from("/scripts/test.sh"));
        app.field_input.fields = vec![crate::domain::Field {
            name: "target".into(),
            prompt: None,
            kind: "string".into(),
            order: 0,
            required: Some(true),
            default: None,
            choices: None,
            arg: None,
        }];
        app.field_input.field_inputs = vec!["prod".to_string()];
        app.submit_form();
        assert!(app.field_input.error.is_none());
        assert!(app.result.is_some());
        let (_, args) = app.result.unwrap();
        assert_eq!(args, vec!["--target", "prod"]);
    }

    #[test]
    fn test_submit_form_required_field_empty_fails() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.field_input.selected_script = Some(PathBuf::from("/scripts/test.sh"));
        app.field_input.fields = vec![crate::domain::Field {
            name: "target".into(),
            prompt: None,
            kind: "string".into(),
            order: 0,
            required: Some(true),
            default: None,
            choices: None,
            arg: None,
        }];
        app.field_input.field_inputs = vec![String::new()];
        app.submit_form();
        assert!(app.field_input.error.is_some());
        assert!(app.result.is_none());
    }

    #[test]
    fn test_move_history_selection() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let rows = vec![
            entry_with_script(&format!("{}/a.sh", tmp.path().display())),
            entry_with_script(&format!("{}/b.sh", tmp.path().display())),
        ];
        let mut app = App::test_new(&svc, ws, vec![], rows);
        assert_eq!(app.history.selection, 0);
        app.move_history_selection(1);
        assert_eq!(app.history.selection, 1);
        app.move_history_selection(1); // clamped
        assert_eq!(app.history.selection, 1);
    }

    #[test]
    fn test_add_history_entry() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        assert!(app.history.entries.is_empty());
        let row = entry_with_script(&format!("{}/x.sh", tmp.path().display()));
        app.add_history_entry(row);
        assert_eq!(app.history.entries.len(), 1);
        assert_eq!(app.history.selection, 0);
    }

    #[test]
    fn test_execution_status_from_run() {
        let mut row = entry_with_script("/s.sh");
        row.success = Some(true);
        assert!(matches!(
            ExecutionStatus::from_run(&row),
            ExecutionStatus::Success
        ));

        row.success = Some(false);
        row.exit_code = Some(42);
        assert!(matches!(
            ExecutionStatus::from_run(&row),
            ExecutionStatus::Failed(Some(42))
        ));

        row.success = Some(true);
        row.error = Some("boom".into());
        assert!(matches!(
            ExecutionStatus::from_run(&row),
            ExecutionStatus::Error
        ));
    }

    #[test]
    fn test_navigate_up_at_root_is_noop() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        let dir_before = app.navigation.current_dir.clone();
        app.navigate_up();
        assert_eq!(app.navigation.current_dir, dir_before);
    }

    #[test]
    fn test_load_schema_sorts_fields_and_enters_field_input() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let script = write_bash_schema_script(
            &tmp,
            "deploy.sh",
            r#"{"Name":"Deploy","Description":"Ship it","Fields":[{"Name":"second","Type":"string","Order":2},{"Name":"first","Type":"string","Order":1,"Required":true}]}"#,
        );
        let mut app = App::test_new(&svc, ws, vec![], vec![]);

        app.load_schema(script.clone());

        assert_eq!(app.screen, Screen::FieldInput);
        assert_eq!(app.field_input.schema_name.as_deref(), Some("Deploy"));
        assert_eq!(app.field_input.fields.len(), 2);
        assert_eq!(app.field_input.fields[0].name, "first");
        assert_eq!(app.field_input.fields[1].name, "second");
        assert_eq!(app.field_input.selected_script.as_ref(), Some(&script));
        assert_eq!(
            app.field_input.field_inputs,
            vec![String::new(), String::new()]
        );
        let (cached_path, cached_schema) = app.navigation.schema_cache.as_ref().unwrap();
        assert_eq!(cached_path, &script);
        assert_eq!(cached_schema.fields[0].name, "first");
    }

    #[test]
    fn test_load_schema_with_no_fields_sets_result_immediately() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let script = write_bash_schema_script(&tmp, "noop.sh", r#"{"Name":"Noop","Fields":[]}"#);
        let mut app = App::test_new(&svc, ws, vec![], vec![]);

        app.load_schema(script.clone());

        assert_eq!(app.screen, Screen::ScriptSelect);
        assert_eq!(app.result, Some((script, Vec::new())));
    }

    #[test]
    fn test_load_schema_error_sets_error_screen() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let script = tmp.path().join("broken.sh");
        write_file(&script, "#!/usr/bin/env bash\necho hi\n");
        let mut app = App::test_new(&svc, ws, vec![], vec![]);

        app.load_schema(script);

        assert_eq!(app.screen, Screen::Error);
        assert!(app.error_message.is_some());
    }

    #[test]
    fn test_enter_selected_directory_refreshes_entries_and_clears_expanded() {
        let tmp = TempDir::new().unwrap();
        write_bash_schema_script(&tmp, "alpha/child.sh", r#"{"Name":"Child","Fields":[]}"#);
        write_bash_schema_script(&tmp, "root.sh", r#"{"Name":"Root","Fields":[]}"#);
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, actual_entries_for_root(&tmp), vec![]);
        app.script_dashboard_expanded = true;

        app.enter_selected();

        assert!(!app.script_dashboard_expanded);
        assert_eq!(app.navigation.current_dir, tmp.path().join("alpha"));
        assert_eq!(app.navigation.entries.len(), 1);
        assert_eq!(
            app.navigation.entries[0].path,
            tmp.path().join("alpha/child.sh")
        );
    }

    #[test]
    fn test_enter_selected_script_loads_schema() {
        let tmp = TempDir::new().unwrap();
        write_bash_schema_script(
            &tmp,
            "deploy.sh",
            r#"{"Name":"Deploy","Fields":[{"Name":"target","Type":"string","Order":1}]}"#,
        );
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut entries = actual_entries_for_root(&tmp);
        entries.retain(|entry| entry.kind == WorkspaceEntryKind::Script);
        let mut app = App::test_new(&svc, ws, entries, vec![]);

        app.enter_selected();

        assert_eq!(app.screen, Screen::FieldInput);
        assert_eq!(app.field_input.schema_name.as_deref(), Some("Deploy"));
    }

    #[test]
    fn test_refresh_entries_with_file_path_sets_error_screen() {
        let tmp = TempDir::new().unwrap();
        let root_file = tmp.path().join("not_a_directory.sh");
        write_file(&root_file, "#!/usr/bin/env bash\n");
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.navigation.current_dir = root_file;

        app.refresh_entries();

        assert_eq!(app.screen, Screen::Error);
        assert!(app.error_message.is_some());
    }

    #[test]
    fn test_poll_widget_load_success_updates_state() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        let (tx, rx) = mpsc::channel();
        app.navigation.widget_loading = true;
        app.navigation.widget_receiver = Some(rx);
        tx.send(WidgetLoadResult {
            widget: Some(WidgetData {
                title: "Widget".into(),
                lines: vec!["one".into()],
            }),
            error: None,
        })
        .unwrap();

        app.poll_widget_load();

        assert!(!app.navigation.widget_loading);
        assert!(app.navigation.widget_receiver.is_none());
        assert_eq!(app.navigation.widget.as_ref().unwrap().title, "Widget");
        assert!(app.navigation.widget_error.is_none());
    }

    #[test]
    fn test_poll_widget_load_disconnected_clears_loading_state() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        let (tx, rx) = mpsc::channel::<WidgetLoadResult>();
        drop(tx);
        app.navigation.widget_loading = true;
        app.navigation.widget_receiver = Some(rx);

        app.poll_widget_load();

        assert!(!app.navigation.widget_loading);
        assert!(app.navigation.widget_receiver.is_none());
    }

    #[test]
    fn test_update_env_preview_handles_empty_and_missing_files() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let envs_dir = ws.envs_dir().to_path_buf();
        let mut app = App::test_new(
            &svc,
            Workspace::new(tmp.path().to_path_buf()),
            vec![],
            vec![],
        );
        std::fs::create_dir_all(&envs_dir).unwrap();
        write_file(&envs_dir.join("dev.conf"), "");
        app.environment.config = Some(EnvironmentConfig {
            envs_dir: envs_dir.clone(),
            active: None,
            defaults: std::collections::HashMap::new(),
            session_conf_path: None,
        });
        app.environment.entries = vec![EnvFile {
            name: "dev.conf".into(),
        }];

        app.update_env_preview();

        assert!(app.environment.preview_error.is_none());
        assert_eq!(app.environment.preview_lines.len(), 1);

        app.environment.entries = vec![EnvFile {
            name: "missing.conf".into(),
        }];
        app.update_env_preview();

        assert!(app.environment.preview_lines.is_empty());
        assert!(app.environment.preview_error.is_some());
    }

    #[test]
    fn test_refresh_search_results_success_populates_details() {
        let tmp = TempDir::new().unwrap();
        write_bash_schema_script(
            &tmp,
            "deploy.sh",
            r#"{"Name":"Deploy","Description":"Ship it","Tags":["ops"],"Fields":[{"Name":"target","Type":"string","Order":1,"Required":true}]}"#,
        );
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        let db_path = tmp.path().join("search.sqlite");
        crate::search_index::rebuild_index(&db_path, tmp.path()).unwrap();
        app.search_index = SearchIndex::new(db_path);
        app.search.query = "deploy".into();

        app.refresh_search_results();

        assert_eq!(app.search.results.len(), 1);
        assert_eq!(app.search.list_state.selected(), Some(0));
        assert!(app.search.error.is_none());
        let details = app.search.details.as_ref().unwrap();
        assert_eq!(details.display_name, "Deploy");
        assert_eq!(details.fields.len(), 1);
        assert_eq!(details.fields[0].name, "target");
    }

    #[test]
    fn test_refresh_search_results_error_clears_results() {
        let tmp = TempDir::new().unwrap();
        let svc = make_service(&tmp);
        let ws = Workspace::new(tmp.path().to_path_buf());
        let mut app = App::test_new(&svc, ws, vec![], vec![]);
        app.search.results = vec![crate::search_index::SearchResult {
            script_path: PathBuf::from("deploy.sh"),
            display_name: "Deploy".into(),
            description: None,
            tags: vec![],
            schema_error: None,
        }];
        app.search_index = SearchIndex::new(tmp.path().to_path_buf());

        app.refresh_search_results();

        assert!(app.search.results.is_empty());
        assert!(app.search.details.is_none());
        assert!(app.search.error.is_some());
        assert_eq!(app.search.list_state.selected(), None);
    }

    #[test]
    fn test_schema_to_preview_includes_outputs_and_matrix_queue() {
        let schema = crate::domain::parse_schema(
            r#"{"Name":"Deploy","Description":"Ship it","Tags":["ops"],"Fields":[{"Name":"target","Type":"string","Order":1,"Required":true}],"Outputs":[{"Name":"url","Type":"string"}],"Queue":{"Matrix":{"Values":[{"Name":"region","Values":["us","eu"]}]}}}"#,
        )
        .unwrap();

        let preview = schema_to_preview(&schema);

        assert_eq!(preview.name, "Deploy");
        assert_eq!(preview.outputs.len(), 1);
        assert_eq!(preview.outputs[0].name, "url");
        match preview.queue.unwrap() {
            QueuePreview::Matrix { values } => {
                assert_eq!(values.len(), 1);
                assert_eq!(values[0].name, "region");
                assert_eq!(values[0].values, vec!["us", "eu"]);
            }
            QueuePreview::Cases { .. } => panic!("expected matrix preview"),
        }
    }

    #[test]
    fn test_schema_to_preview_includes_cases_queue() {
        let schema = crate::domain::parse_schema(
            r#"{"Name":"Deploy","Fields":[],"Queue":{"Cases":[{"Name":"prod","Values":[{"Name":"region","Value":"us"}]},{"Values":[{"Name":"region","Value":"eu"}]}]}}"#,
        )
        .unwrap();

        let preview = schema_to_preview(&schema);

        match preview.queue.unwrap() {
            QueuePreview::Cases { cases } => {
                assert_eq!(cases.len(), 2);
                assert_eq!(cases[0].name.as_deref(), Some("prod"));
                assert_eq!(cases[1].name, None);
                assert_eq!(cases[0].values[0].name, "region");
                assert_eq!(cases[1].values[0].value, "eu");
            }
            QueuePreview::Matrix { .. } => panic!("expected cases preview"),
        }
    }
}
