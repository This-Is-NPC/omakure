use crate::adapters::environments::{self, EnvFile, EnvironmentConfig};
use crate::domain::Schema;
use crate::history::HistoryEntry;
use crate::lua_widget::{self, WidgetData};
use crate::ports::{WorkspaceEntry, WorkspaceEntryKind};
use crate::search_index::{SearchDetails, SearchIndex, SearchResult, SearchStatus};
use crate::use_cases::ScriptService;
use crate::workspace::Workspace;
use ratatui::widgets::{ListState, TableState};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum HistoryFocus {
    List,
    Output,
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
    pub(crate) current_dir: PathBuf,
    pub(crate) entries: Vec<WorkspaceEntry>,
    pub(crate) widget: Option<WidgetData>,
    pub(crate) widget_error: Option<String>,
    pub(crate) widget_loading: bool,
    widget_receiver: Option<Receiver<WidgetLoadResult>>,
    pub(crate) env_config: Option<EnvironmentConfig>,
    pub(crate) env_error: Option<String>,
    pub(crate) env_entries: Vec<EnvFile>,
    pub(crate) env_state: ListState,
    env_selection: usize,
    pub(crate) env_preview_lines: Vec<ratatui::text::Line<'static>>,
    pub(crate) env_preview_error: Option<String>,
    pub(crate) env_preview_scroll: u16,
    pub(crate) schema_preview: Option<SchemaPreview>,
    pub(crate) schema_preview_error: Option<String>,
    preview_script: Option<PathBuf>,
    schema_cache: Option<(PathBuf, Schema)>,
    pub(crate) list_state: ListState,
    selection: usize,
    pub(crate) history: Vec<HistoryEntry>,
    pub(crate) history_state: TableState,
    history_selection: usize,
    pub(crate) history_focus: HistoryFocus,
    pub(crate) screen: Screen,
    env_return: Option<Screen>,
    search_index: SearchIndex,
    pub(crate) search_query: String,
    pub(crate) search_results: Vec<SearchResult>,
    pub(crate) search_state: ListState,
    search_selection: usize,
    pub(crate) search_details: Option<SearchDetails>,
    pub(crate) search_status: SearchStatus,
    pub(crate) search_error: Option<String>,
    pub(crate) schema_name: Option<String>,
    pub(crate) schema_description: Option<String>,
    pub(crate) fields: Vec<crate::domain::Field>,
    pub(crate) field_index: usize,
    pub(crate) field_inputs: Vec<String>,
    pub(crate) args: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) selected_script: Option<PathBuf>,
    pub(crate) result: Option<(PathBuf, Vec<String>)>,
    pub(crate) should_quit: bool,
    pub(crate) run_output_scroll: u16,
}

impl<'a> App<'a> {
    pub(crate) fn new(
        service: &'a ScriptService,
        workspace: Workspace,
        entries: Vec<WorkspaceEntry>,
        history: Vec<HistoryEntry>,
        search_index: SearchIndex,
    ) -> Self {
        let mut list_state = ListState::default();
        if !entries.is_empty() {
            list_state.select(Some(0));
        }
        let mut history_state = TableState::default();
        if !history.is_empty() {
            history_state.select(Some(0));
        }
        let current_dir = workspace.root().to_path_buf();
        let search_status = search_index.status();
        let mut app = Self {
            service,
            workspace,
            current_dir,
            entries,
            widget: None,
            widget_error: None,
            widget_loading: false,
            widget_receiver: None,
            env_config: None,
            env_error: None,
            env_entries: Vec::new(),
            env_state: ListState::default(),
            env_selection: 0,
            env_preview_lines: Vec::new(),
            env_preview_error: None,
            env_preview_scroll: 0,
            schema_preview: None,
            schema_preview_error: None,
            preview_script: None,
            schema_cache: None,
            list_state,
            selection: 0,
            history,
            history_state,
            history_selection: 0,
            history_focus: HistoryFocus::List,
            screen: Screen::ScriptSelect,
            env_return: None,
            search_index,
            search_query: String::new(),
            search_results: Vec::new(),
            search_state: ListState::default(),
            search_selection: 0,
            search_details: None,
            search_status,
            search_error: None,
            schema_name: None,
            schema_description: None,
            fields: Vec::new(),
            field_index: 0,
            field_inputs: Vec::new(),
            args: Vec::new(),
            error: None,
            selected_script: None,
            result: None,
            should_quit: false,
            run_output_scroll: 0,
        };
        app.start_widget_load();
        app.load_env_config();
        app.update_schema_preview();
        app.update_env_preview();
        app
    }

    pub(crate) fn selected_entry(&self) -> Option<&WorkspaceEntry> {
        self.entries.get(self.selection)
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let mut new_index = self.selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.selection = new_index as usize;
        self.list_state.select(Some(self.selection));
        self.update_schema_preview();
    }

    pub(crate) fn enter_search(&mut self) {
        self.search_status = self.search_index.status();
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
        let mut next = self.env_preview_scroll as i16 + delta;
        if next < 0 {
            next = 0;
        }
        if next > u16::MAX as i16 {
            next = u16::MAX as i16;
        }
        self.env_preview_scroll = next as u16;
    }

    pub(crate) fn move_env_selection(&mut self, delta: isize) {
        if self.env_entries.is_empty() {
            return;
        }
        let len = self.env_entries.len() as isize;
        let mut new_index = self.env_selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.env_selection = new_index as usize;
        self.env_state.select(Some(self.env_selection));
        self.update_env_preview();
    }

    pub(crate) fn activate_selected_env(&mut self) {
        if self.env_entries.is_empty() {
            return;
        }
        let name = self.env_entries[self.env_selection].name.clone();
        match environments::set_active_env(self.workspace.envs_dir(), Some(&name)) {
            Ok(()) => self.load_env_config(),
            Err(err) => self.env_error = Some(err),
        }
    }

    pub(crate) fn deactivate_env(&mut self) {
        match environments::set_active_env(self.workspace.envs_dir(), None) {
            Ok(()) => self.load_env_config(),
            Err(err) => self.env_error = Some(err),
        }
    }

    pub(crate) fn refresh_search_status(&mut self) {
        let status = self.search_index.status();
        if status != self.search_status {
            self.search_status = status.clone();
            if self.screen == Screen::Search {
                self.refresh_search_results();
            }
        }
    }

    pub(crate) fn move_search_selection(&mut self, delta: isize) {
        if self.search_results.is_empty() {
            return;
        }
        let len = self.search_results.len() as isize;
        let mut new_index = self.search_selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.search_selection = new_index as usize;
        self.search_state.select(Some(self.search_selection));
        self.update_search_details();
    }

    pub(crate) fn append_search_char(&mut self, ch: char) {
        self.search_query.push(ch);
        self.refresh_search_results();
    }

    pub(crate) fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.refresh_search_results();
    }

    pub(crate) fn open_selected_search(&mut self) {
        let entry = match self.search_results.get(self.search_selection) {
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
                self.current_dir = entry.path;
                self.refresh_entries();
            }
            WorkspaceEntryKind::Script => {
                self.load_schema(entry.path);
            }
        }
    }

    pub(crate) fn navigate_up(&mut self) {
        if self.current_dir == self.workspace.root() {
            return;
        }
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.refresh_entries();
        }
    }

    pub(crate) fn move_history_selection(&mut self, delta: isize) {
        if self.history.is_empty() {
            return;
        }
        let len = self.history.len() as isize;
        let mut new_index = self.history_selection as isize + delta;
        if new_index < 0 {
            new_index = 0;
        } else if new_index >= len {
            new_index = len - 1;
        }
        self.history_selection = new_index as usize;
        self.history_state.select(Some(self.history_selection));
        self.reset_run_output_scroll();
    }

    pub(crate) fn add_history_entry(&mut self, entry: HistoryEntry) {
        self.history.insert(0, entry);
        self.history_selection = 0;
        self.history_state.select(Some(0));
    }

    pub(crate) fn current_history_entry(&self) -> Option<&HistoryEntry> {
        self.history.get(self.history_selection)
    }

    pub(crate) fn load_schema(&mut self, script: PathBuf) {
        let schema_result = match self.schema_cache.as_ref() {
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
                self.schema_name = Some(schema.name);
                self.schema_description = schema.description;
                self.fields = schema.fields;
                self.field_index = 0;
                self.field_inputs = self.build_field_inputs();
                self.args.clear();
                self.error = None;
                self.selected_script = Some(script.clone());
                self.schema_cache = Some((
                    script.clone(),
                    Schema {
                        name: self.schema_name.clone().unwrap_or_default(),
                        description: self.schema_description.clone(),
                        tags,
                        fields: self.fields.clone(),
                        outputs,
                        queue,
                    },
                ));
                if self.fields.is_empty() {
                    self.result = Some((script, Vec::new()));
                } else {
                    self.screen = Screen::FieldInput;
                }
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.screen = Screen::Error;
            }
        }
    }

    pub(crate) fn move_field_selection(&mut self, delta: isize) {
        if self.fields.is_empty() {
            return;
        }
        let len = self.fields.len() as isize;
        let mut new_index = self.field_index as isize + delta;
        while new_index < 0 {
            new_index += len;
        }
        while new_index >= len {
            new_index -= len;
        }
        self.field_index = new_index as usize;
        self.error = None;
    }

    pub(crate) fn append_field_char(&mut self, ch: char) {
        if let Some(value) = self.field_inputs.get_mut(self.field_index) {
            value.push(ch);
            self.error = None;
        }
    }

    pub(crate) fn pop_field_char(&mut self) {
        if let Some(value) = self.field_inputs.get_mut(self.field_index) {
            value.pop();
            self.error = None;
        }
    }

    pub(crate) fn submit_form(&mut self) {
        if self.fields.is_empty() {
            self.finish();
            return;
        }

        let mut args = Vec::new();
        for (idx, field) in self.fields.iter().enumerate() {
            let input = self.field_inputs.get(idx).map(String::as_str).unwrap_or("");
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
                    self.error = Some(format!("{}: {}", field.name, message));
                    self.field_index = idx;
                    return;
                }
            }
        }

        self.args = args;
        self.error = None;
        self.finish();
    }

    fn finish(&mut self) {
        if let Some(script) = &self.selected_script {
            self.result = Some((script.clone(), self.args.clone()));
        } else {
            self.should_quit = true;
        }
    }

    pub(crate) fn refresh_entries(&mut self) {
        match self.service.list_entries(&self.current_dir) {
            Ok(entries) => {
                self.entries = entries;
                self.selection = 0;
                if self.entries.is_empty() {
                    self.list_state.select(None);
                } else {
                    self.list_state.select(Some(0));
                }
                self.error = None;
                self.start_widget_load();
                self.update_schema_preview();
            }
            Err(err) => {
                self.error = Some(err.to_string());
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
        self.schema_name = None;
        self.schema_description = None;
        self.fields.clear();
        self.field_index = 0;
        self.field_inputs.clear();
        self.args.clear();
        self.error = None;
        self.selected_script = None;
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
        path.strip_prefix(self.workspace.root())
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    }

    fn start_widget_load(&mut self) {
        let dir = self.current_dir.clone();
        let (tx, rx) = mpsc::channel();
        self.widget_loading = true;
        self.widget = None;
        self.widget_error = None;
        self.widget_receiver = Some(rx);
        std::thread::spawn(move || {
            let (widget, error) = load_widget_state(&dir);
            let _ = tx.send(WidgetLoadResult { widget, error });
        });
    }

    pub(crate) fn poll_widget_load(&mut self) {
        let Some(receiver) = &self.widget_receiver else {
            return;
        };

        match receiver.try_recv() {
            Ok(result) => {
                self.widget = result.widget;
                self.widget_error = result.error;
                self.widget_loading = false;
                self.widget_receiver = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.widget_loading = false;
                self.widget_receiver = None;
            }
        }
    }

    fn load_env_config(&mut self) {
        let envs_dir = self.workspace.envs_dir();
        let mut env_error = None;

        let env_config = match environments::load_environment_config(envs_dir) {
            Ok(config) => Some(config),
            Err(err) => {
                env_error = Some(err);
                None
            }
        };

        let env_entries = match environments::list_env_files(envs_dir) {
            Ok(entries) => entries,
            Err(err) => {
                if env_error.is_none() {
                    env_error = Some(err);
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
            self.env_selection.min(env_entries.len().saturating_sub(1))
        };

        self.env_entries = env_entries;
        self.env_selection = selected;
        if self.env_entries.is_empty() {
            self.env_state.select(None);
        } else {
            self.env_state.select(Some(self.env_selection));
        }

        self.env_config = env_config;
        self.env_error = env_error;
        self.update_env_preview();
    }

    fn update_env_preview(&mut self) {
        self.env_preview_scroll = 0;
        self.env_preview_error = None;

        let entry = match self.env_entries.get(self.env_selection) {
            Some(entry) => entry,
            None => {
                self.env_preview_lines = Vec::new();
                return;
            }
        };

        let envs_dir = self
            .env_config
            .as_ref()
            .map(|config| config.envs_dir.clone())
            .unwrap_or_else(|| self.workspace.envs_dir().to_path_buf());
        let env_path = envs_dir.join(&entry.name);

        match environments::load_env_preview(&env_path) {
            Ok(entries) => {
                let mut lines = Vec::new();
                for (key, value) in entries {
                    let line = ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(
                            key,
                            ratatui::style::Style::default()
                                .fg(ratatui::style::Color::Yellow)
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        ratatui::text::Span::styled(
                            " = ",
                            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
                        ),
                        ratatui::text::Span::raw(value),
                    ]);
                    lines.push(line);
                }
                if lines.is_empty() {
                    self.env_preview_lines =
                        vec![ratatui::text::Line::from(ratatui::text::Span::styled(
                            "No entries found.",
                            ratatui::style::Style::default().fg(ratatui::style::Color::Gray),
                        ))];
                } else {
                    self.env_preview_lines = lines;
                }
                self.env_preview_error = None;
            }
            Err(err) => {
                self.env_preview_lines = Vec::new();
                self.env_preview_error = Some(err);
            }
        }
    }

    fn build_field_inputs(&self) -> Vec<String> {
        let defaults = self.env_config.as_ref().map(|config| &config.defaults);
        match defaults {
            Some(defaults) if !defaults.is_empty() => self
                .fields
                .iter()
                .map(|field| {
                    defaults
                        .get(&field.name.to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_default()
                })
                .collect(),
            _ => vec![String::new(); self.fields.len()],
        }
    }

    fn update_schema_preview(&mut self) {
        let (entry_path, entry_kind) = match self.selected_entry() {
            Some(entry) => (entry.path.clone(), entry.kind),
            None => {
                self.schema_preview = None;
                self.schema_preview_error = None;
                self.preview_script = None;
                return;
            }
        };

        if entry_kind != WorkspaceEntryKind::Script {
            self.schema_preview = None;
            self.schema_preview_error = None;
            self.preview_script = None;
            return;
        }

        if self.preview_script.as_ref() == Some(&entry_path) {
            return;
        }

        match self.service.load_schema(&entry_path) {
            Ok(mut schema) => {
                schema.fields.sort_by_key(|field| field.order);
                self.schema_preview = Some(schema_to_preview(&schema));
                self.schema_preview_error = None;
                self.preview_script = Some(entry_path.clone());
                self.schema_cache = Some((entry_path, schema));
            }
            Err(err) => {
                self.schema_preview = None;
                self.schema_preview_error = Some(err.to_string());
                self.preview_script = Some(entry_path);
            }
        }
    }

    fn refresh_search_results(&mut self) {
        match self.search_index.query(&self.search_query) {
            Ok(results) => {
                self.search_results = results;
                self.search_error = None;
            }
            Err(err) => {
                self.search_results.clear();
                self.search_error = Some(err);
            }
        }
        self.search_selection = 0;
        if self.search_results.is_empty() {
            self.search_state.select(None);
        } else {
            self.search_state.select(Some(0));
        }
        self.update_search_details();
    }

    fn update_search_details(&mut self) {
        self.search_details = None;
        let entry = match self.search_results.get(self.search_selection) {
            Some(entry) => entry,
            None => return,
        };
        match self.search_index.load_details(&entry.script_path) {
            Ok(details) => {
                self.search_details = details;
                self.search_error = None;
            }
            Err(err) => {
                self.search_error = Some(err);
            }
        }
    }
}

impl ExecutionStatus {
    pub(crate) fn from_history(entry: &HistoryEntry) -> Self {
        if entry.error.is_some() {
            ExecutionStatus::Error
        } else if entry.success {
            ExecutionStatus::Success
        } else {
            ExecutionStatus::Failed(entry.exit_code)
        }
    }
}

struct WidgetLoadResult {
    widget: Option<WidgetData>,
    error: Option<String>,
}

fn load_widget_state(dir: &Path) -> (Option<WidgetData>, Option<String>) {
    match lua_widget::load_widget(dir) {
        Ok(widget) => (widget, None),
        Err(err) => (None, Some(err)),
    }
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
