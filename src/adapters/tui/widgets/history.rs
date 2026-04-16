use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;
use std::path::PathBuf;

use super::super::app::{App, ExecutionStatus, HistoryFocus, HistoryView};
use super::super::theme::Theme;
use super::common::{state_style, status_label_and_style};
use super::dashboards::render_dashboards;
use crate::runs::{format_run_timestamp, RunRow};

pub(crate) fn render_history(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    match app.history.view {
        HistoryView::List => {
            let list_width = history_list_width(chunks[0].width, app);
            let body_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(list_width), Constraint::Min(10)])
                .split(chunks[0]);

            render_history_list(frame, body_chunks[0], app, theme);
            render_history_output(frame, body_chunks[1], app, theme);
        }
        HistoryView::Dashboards => {
            render_dashboards(frame, chunks[0], app, theme);
        }
    }

    let footer_text = match app.history.view {
        HistoryView::List => match app.history.focus {
            HistoryFocus::List => {
                "Tab dashboards, Up/Down select, Enter view output, Alt+E envs, Esc/q back"
            }
            HistoryFocus::Output => "Tab dashboards, Up/Down scroll, PgUp/PgDn, Esc return, q back",
        },
        HistoryView::Dashboards => {
            "Tab list, Up/Down select script, e/Enter expand, Alt+E envs, Esc/q back"
        }
    };
    let footer = Paragraph::new(footer_text).style(theme.text_secondary());
    frame.render_widget(footer, chunks[1]);
}

fn render_history_list(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if app.history.entries.is_empty() {
        let empty = Paragraph::new("No executions yet.")
            .block(Block::default().borders(Borders::ALL).title("History"))
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let rows: Vec<Row> = app
        .history
        .entries
        .iter()
        .map(|entry| {
            let name = app.display_path(&PathBuf::from(&entry.script_path));
            let name = if matches!(entry.trigger, crate::runs::RunTrigger::Scheduled) {
                format!("⏰ {name}")
            } else {
                name
            };
            let date = format_run_timestamp(entry.started_at.unwrap_or(entry.enqueued_at));
            let state_label = entry.state.as_str();
            let state_text_style = state_style(theme, entry.state);
            let status = ExecutionStatus::from_run(entry);
            let (status_label, status_style) = status_label_and_style(&status, theme);
            Row::new(vec![
                Cell::from(Span::styled(state_label, state_text_style)),
                Cell::from(Span::styled(status_label, status_style)),
                Cell::from(Span::raw(date)),
                Cell::from(Span::raw(name)),
                Cell::from(Span::raw(entry.actor.clone())),
            ])
        })
        .collect();

    let header = Row::new(vec![
        Cell::from(Span::styled("State", theme.text_secondary())),
        Cell::from(Span::styled("Status", theme.text_secondary())),
        Cell::from(Span::styled("Date", theme.text_secondary())),
        Cell::from(Span::styled("Script", theme.text_secondary())),
        Cell::from(Span::styled("Actor", theme.text_secondary())),
    ]);
    let highlight_style = match app.history.focus {
        HistoryFocus::List => theme.selection_style(),
        HistoryFocus::Output => theme.text_muted(),
    };
    let highlight_symbol = if app.history.focus == HistoryFocus::List {
        theme.selection_symbol()
    } else {
        Span::styled("> ", highlight_style)
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(HISTORY_STATE_WIDTH),
            Constraint::Length(HISTORY_STATUS_WIDTH),
            Constraint::Length(HISTORY_DATE_WIDTH),
            Constraint::Min(HISTORY_MIN_SCRIPT_WIDTH),
            Constraint::Length(HISTORY_ACTOR_WIDTH),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title("History"))
    .highlight_style(highlight_style)
    .highlight_symbol(highlight_symbol);

    frame.render_stateful_widget(table, area, &mut app.history.table_state);
}

fn render_history_output(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let mut lines = Vec::new();
    if let Some(entry) = app.current_history_entry() {
        let name = app.display_path(&PathBuf::from(&entry.script_path));
        let args = format_args_human(&entry.args_json);
        let status = ExecutionStatus::from_run(entry);
        let (status_label, status_style) = status_label_and_style(&status, theme);
        lines.push(Line::from(format!("Script: {}", name)));
        lines.push(Line::from(format!("Args: {}", args)));
        lines.push(Line::from(vec![
            Span::raw("Status: "),
            Span::styled(status_label, status_style),
        ]));
        lines.push(Line::from(format!("Actor: {}", entry.actor)));
        if let Some(reason) = &entry.reason {
            lines.push(Line::from(format!("Reason: {}", reason)));
        }
        lines.push(Line::from(format!("Run id: {}", entry.run_id)));
        lines.push(Line::from(""));
        let output = format_run_output(entry);
        if output.trim().is_empty() {
            lines.push(Line::from("(no output)"));
        } else {
            lines.extend(output.lines().map(|line| Line::from(line.to_string())));
        }
    } else {
        lines.push(Line::from("No history selected."));
    }

    let view_height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(view_height);
    if max_scroll == 0 {
        app.run_output_scroll = 0;
    } else if app.run_output_scroll as usize > max_scroll {
        app.run_output_scroll = max_scroll.min(u16::MAX as usize) as u16;
    }

    let mut block = Block::default().borders(Borders::ALL).title("Output");
    if app.history.focus == HistoryFocus::Output {
        let border_style = theme.selection_border_style();
        block = block.border_style(border_style).title_style(border_style);
    }

    let output = Paragraph::new(lines)
        .block(block)
        .style(Style::default())
        .wrap(Wrap { trim: false })
        .scroll((app.run_output_scroll, 0));
    frame.render_widget(output, area);
}

/// Format the stdout/stderr/error trio of a [`RunRow`] into a single
/// human-readable string for the TUI history screen.
pub(crate) fn format_run_output(entry: &RunRow) -> String {
    if let Some(error) = &entry.error {
        return error.trim().to_string();
    }
    let mut parts = Vec::new();
    if !entry.stdout.trim().is_empty() {
        parts.push(format!("STDOUT:\n{}", entry.stdout.trim_end()));
    }
    if !entry.stderr.trim().is_empty() {
        parts.push(format!("STDERR:\n{}", entry.stderr.trim_end()));
    }
    parts.join("\n\n")
}

fn format_args_human(args_json: &str) -> String {
    match serde_json::from_str::<Vec<String>>(args_json) {
        Ok(args) if args.is_empty() => "-".to_string(),
        Ok(args) => args.join(" "),
        Err(_) => args_json.to_string(),
    }
}

const HISTORY_STATE_WIDTH: u16 = 12;
const HISTORY_STATUS_WIDTH: u16 = 10;
const HISTORY_DATE_WIDTH: u16 = 16;
const HISTORY_MIN_SCRIPT_WIDTH: u16 = 10;
const HISTORY_ACTOR_WIDTH: u16 = 8;
const HISTORY_COLUMN_SPACING: u16 = 1;
const HISTORY_HIGHLIGHT_WIDTH: u16 = 2;
const HISTORY_BORDER_WIDTH: u16 = 2;
const HISTORY_MIN_OUTPUT_WIDTH: u16 = 30;

fn history_list_width(total_width: u16, app: &App) -> u16 {
    let max_script = app
        .history
        .entries
        .iter()
        .map(|entry| app.display_path(&PathBuf::from(&entry.script_path)).len() as u16)
        .max()
        .unwrap_or(0)
        .max(HISTORY_MIN_SCRIPT_WIDTH);

    let content_width = HISTORY_STATE_WIDTH
        + HISTORY_STATUS_WIDTH
        + HISTORY_DATE_WIDTH
        + max_script
        + HISTORY_ACTOR_WIDTH
        + HISTORY_COLUMN_SPACING * 4;
    let desired = content_width + HISTORY_BORDER_WIDTH + HISTORY_HIGHLIGHT_WIDTH;
    let min_output = HISTORY_MIN_OUTPUT_WIDTH.min(total_width.saturating_sub(10).max(1));
    let max_list = total_width.saturating_sub(min_output);
    desired.min(max_list).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::RunState;

    fn row(stdout: &str, stderr: &str, error: Option<&str>) -> RunRow {
        RunRow {
            run_id: "rid".into(),
            script_path: "/x/a.sh".into(),
            script_name: None,
            args_json: "[]".into(),
            actor: "human".into(),
            reason: None,
            state: RunState::Completed,
            priority: 0,
            enqueued_at: 0,
            worker_id: None,
            lease_until: None,
            timeout_ms: None,
            cron_schedule_id: None,
            trigger: crate::runs::RunTrigger::Manual,
            started_at: Some(0),
            finished_at: Some(0),
            duration_ms: Some(0),
            exit_code: Some(0),
            success: Some(true),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            error: error.map(|s| s.to_string()),
            parent_run_id: None,
            omakure_version: "test".into(),
        }
    }

    #[test]
    fn format_run_output_prefers_error() {
        let r = row("ignored", "ignored", Some("boom"));
        assert_eq!(format_run_output(&r), "boom");
    }

    #[test]
    fn format_run_output_combines_stdout_and_stderr() {
        let r = row("hello\n", "warning\n", None);
        let s = format_run_output(&r);
        assert!(s.contains("STDOUT:"));
        assert!(s.contains("hello"));
        assert!(s.contains("STDERR:"));
        assert!(s.contains("warning"));
    }

    #[test]
    fn format_args_human_handles_empty_and_filled() {
        assert_eq!(format_args_human("[]"), "-");
        assert_eq!(format_args_human(r#"["--foo","bar"]"#), "--foo bar");
        // Garbage falls back to raw string.
        assert_eq!(format_args_human("not-json"), "not-json");
    }

    #[test]
    fn format_run_output_empty_stdout_stderr() {
        let r = row("", "", None);
        assert_eq!(format_run_output(&r), "");
    }

    #[test]
    fn format_run_output_only_stdout() {
        let r = row("output\n", "", None);
        let s = format_run_output(&r);
        assert!(s.contains("STDOUT:"));
        assert!(!s.contains("STDERR:"));
    }

    // --- Rendering tests ---

    use crate::adapters::script_runner::MultiScriptRunner;
    use crate::adapters::workspace_repository::FsWorkspaceRepository;
    use crate::use_cases::ScriptService;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tempfile::TempDir;

    fn history_row(tmp: &TempDir, name: &str, state: RunState) -> RunRow {
        RunRow {
            run_id: format!("rid-{}", name),
            script_path: format!("{}/{}", tmp.path().display(), name),
            script_name: None,
            args_json: "[]".into(),
            actor: "human".into(),
            reason: None,
            state,
            priority: 0,
            enqueued_at: 1000,
            worker_id: None,
            lease_until: None,
            timeout_ms: None,
            cron_schedule_id: None,
            trigger: crate::runs::RunTrigger::Manual,
            started_at: Some(1000),
            finished_at: Some(1100),
            duration_ms: Some(100),
            exit_code: Some(0),
            success: Some(true),
            stdout: "ok\n".to_string(),
            stderr: String::new(),
            error: None,
            parent_run_id: None,
            omakure_version: "test".into(),
        }
    }

    #[test]
    fn snapshot_render_history_list() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let entries = vec![
            history_row(&tmp, "deploy.sh", RunState::Completed),
            history_row(&tmp, "setup.sh", RunState::Failed),
        ];
        let mut app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], entries);
        app.screen = crate::adapters::tui::app::Screen::History;
        let theme = app.theme.clone();

        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_history(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }

    #[test]
    fn render_history_dashboards_view_no_panic() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let entries = vec![history_row(&tmp, "deploy.sh", RunState::Completed)];
        let mut app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], entries);
        app.screen = crate::adapters::tui::app::Screen::History;
        app.history.view = HistoryView::Dashboards;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_history(f, f.size(), &mut app, &theme))
            .unwrap();
    }

    #[test]
    fn render_history_output_focus_with_reason_and_scroll() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut row = history_row(&tmp, "deploy.sh", RunState::Completed);
        row.reason = Some("manual retry".into());
        // Make the output much taller than the rendered area so the scroll
        // clamp branches fire.
        row.stdout = (0..50).map(|i| format!("line {}\n", i)).collect();
        let mut app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], vec![row]);
        app.screen = crate::adapters::tui::app::Screen::History;
        app.history.focus = HistoryFocus::Output;
        app.run_output_scroll = u16::MAX;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_history(f, f.size(), &mut app, &theme))
            .unwrap();
    }

    #[test]
    fn render_history_empty_output_branch_no_history_entry_fallback() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let row = history_row(&tmp, "deploy.sh", RunState::Completed);
        // Empty output triggers the "(no output)" branch.
        let mut row = row;
        row.stdout.clear();
        row.stderr.clear();
        let mut app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], vec![row]);
        app.screen = crate::adapters::tui::app::Screen::History;
        let theme = app.theme.clone();
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_history(f, f.size(), &mut app, &theme))
            .unwrap();
    }

    #[test]
    fn snapshot_render_history_empty() {
        let tmp = TempDir::new().unwrap();
        let repo = FsWorkspaceRepository::new(tmp.path());
        let runner = MultiScriptRunner::new();
        let svc = ScriptService::new(Box::new(repo), Box::new(runner));
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        let mut app = crate::adapters::tui::app::App::test_new(&svc, ws, vec![], vec![]);
        app.screen = crate::adapters::tui::app::Screen::History;
        let theme = app.theme.clone();

        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_history(f, f.size(), &mut app, &theme))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
